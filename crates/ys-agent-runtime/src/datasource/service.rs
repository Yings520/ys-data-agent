use std::{
    collections::HashSet,
    num::NonZeroU64,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use chrono::Utc;
use ys_agent_core::*;

use super::SourcePolicy;

pub struct DatasourceService {
    repository: Arc<dyn DatasourceRepository>,
    vault: Arc<dyn DatasourceVault>,
    catalog: Arc<dyn ConnectorCatalog>,
    policy: Arc<SourcePolicy>,
    cancelled: Mutex<HashSet<OperationId>>,
}

impl DatasourceService {
    pub fn new(
        repository: Arc<dyn DatasourceRepository>,
        vault: Arc<dyn DatasourceVault>,
        catalog: Arc<dyn ConnectorCatalog>,
        policy: Arc<SourcePolicy>,
    ) -> Self {
        Self {
            repository,
            vault,
            catalog,
            policy,
            cancelled: Mutex::new(HashSet::new()),
        }
    }

    fn descriptor(
        &self,
        adapter: &AdapterId,
        version: &AdapterVersion,
    ) -> DsResult<ConnectorDescriptor> {
        self.catalog
            .descriptors()?
            .into_iter()
            .find(|descriptor| {
                &descriptor.adapter_id == adapter && &descriptor.adapter_version == version
            })
            .ok_or_else(|| error(DsErrorCode::ConfigIncompatible))
    }

    fn capability(
        &self,
        descriptor: &ConnectorDescriptor,
        source: &SourceId,
        governance: &DatasourceGovernanceContext,
    ) -> CapabilityDescriptor {
        let mut capability = descriptor.capability.clone();
        capability.source_id = source.clone();
        capability.max_concurrency = capability
            .max_concurrency
            .min(governance.budget.max_concurrency);
        capability
    }

    async fn open_revision(
        &self,
        detail: &DatasourceDetail,
    ) -> DsResult<(
        Arc<dyn ManagedConnector>,
        DatasourceValidationInputs,
        AdapterVersion,
    )> {
        let revision = &detail.revision;
        let input = revision.input();
        let descriptor = self.descriptor(&input.adapter_id, &input.adapter_version)?;
        let factory = self
            .catalog
            .factory(&input.adapter_id, &input.adapter_version)?;
        let issues = factory.validate_config(revision);
        if let Some(issue) = issues.first() {
            return Err(field_error(DsErrorCode::InvalidField, issue.field.clone()));
        }
        let governance = self.policy.governance_for(revision)?;
        let source = input
            .source_id
            .as_ref()
            .ok_or_else(|| error(DsErrorCode::PolicyDenied))?;
        let capability = self.capability(&descriptor, source, &governance);
        let validation_inputs = DatasourceValidationInputs::new(
            revision,
            &capability,
            governance.policy_digest.clone(),
        )
        .map_err(|_| error(DsErrorCode::ValidationStale))?;
        let secret = match input.credential {
            Some(reference) => Some(self.vault.read(reference).await?),
            None => None,
        };
        let connector = factory
            .open(ConnectorOpenInput {
                revision: revision.clone(),
                secret,
                governance,
            })
            .await?;
        Ok((connector, validation_inputs, descriptor.adapter_version))
    }

    fn was_cancelled(&self, operation: OperationId) -> bool {
        self.cancelled
            .lock()
            .is_ok_and(|cancelled| cancelled.contains(&operation))
    }

    async fn cleanup_committed_mutation(&self, mutation: &SecretMutation) {
        if let Some(reference) = mutation.old {
            match self.repository.claim_secret_cleanup(reference).await {
                Ok(()) => {
                    if self.vault.remove(reference).await.is_ok() {
                        let _ = self.repository.finish_secret_cleanup(reference).await;
                    }
                }
                Err(error) if error.code == DsErrorCode::InUse => {}
                Err(_) => return,
            }
        }
        let _ = self
            .repository
            .finish_secret_mutation(mutation.mutation_id)
            .await;
    }

    async fn rollback_uncommitted_mutation(&self, mutation: &SecretMutation) {
        if let Some(reference) = mutation.new
            && self
                .repository
                .claim_secret_cleanup(reference)
                .await
                .is_ok()
        {
            let _ = self.vault.remove(reference).await;
            let _ = self.repository.finish_secret_cleanup(reference).await;
        }
        let _ = self
            .repository
            .finish_secret_mutation(mutation.mutation_id)
            .await;
    }
}

#[async_trait]
impl DatasourceManagementApi for DatasourceService {
    async fn view(&self, scope: DatasourceScope) -> DsResult<DatasourceView> {
        Ok(DatasourceView {
            schema_version: 1,
            catalog: self.catalog.descriptors()?,
            snapshot: self.repository.load(scope).await?,
        })
    }

    async fn save(&self, request: SaveDatasource) -> DsResult<DatasourceDetail> {
        if let Some(receipt) = self.repository.receipt(request.write.command_id).await? {
            return replayed_save(receipt, &request);
        }
        let descriptor = self.descriptor(&request.adapter_id, &request.adapter_version)?;
        if descriptor.config_version != request.config_version {
            return Err(error(DsErrorCode::ConfigIncompatible));
        }
        let snapshot = self.repository.load(request.write.scope).await?;
        let existing = match request.profile_id {
            Some(profile_id) => Some(
                snapshot
                    .profiles
                    .iter()
                    .find(|detail| detail.profile.profile_id == profile_id)
                    .cloned()
                    .ok_or_else(|| error(DsErrorCode::Conflict))?,
            ),
            None => None,
        };
        let profile_id = existing
            .as_ref()
            .map_or_else(ProfileId::new, |detail| detail.profile.profile_id);
        let revision_number = existing
            .as_ref()
            .map_or(1, |detail| detail.revision.number().saturating_add(1));
        let old_reference = existing
            .as_ref()
            .and_then(|detail| detail.revision.input().credential);
        let has_secret = match &request.secret {
            SecretEdit::Keep => old_reference.is_some(),
            SecretEdit::Replace(_) => true,
            SecretEdit::Remove => false,
        };
        let issues =
            validate_datasource_fields(&descriptor.fields, &request.fields, has_secret, false);
        if let Some(issue) = issues.first() {
            return Err(field_error(DsErrorCode::InvalidField, issue.field.clone()));
        }
        if existing.as_ref().is_some_and(|detail| {
            detail.revision.input().adapter_id != request.adapter_id
                || detail.revision.input().adapter_version != request.adapter_version
        }) {
            return Err(error(DsErrorCode::ConfigIncompatible));
        }

        let matched_source = if matches!(request.context, DatabaseContext::Unconfigured) {
            None
        } else {
            self.policy
                .match_target(&request.adapter_id, &request.fields, &request.context)
                .ok()
                .map(|matched| matched.0)
        };
        let source_id = match (
            existing.as_ref().and_then(|d| d.profile.source_id.clone()),
            matched_source,
        ) {
            (Some(existing), Some(matched)) if existing != matched => {
                return Err(error(DsErrorCode::PolicyDenied));
            }
            (Some(existing), _) => Some(existing),
            (None, matched) => matched,
        };
        let new_reference = match &request.secret {
            SecretEdit::Keep => old_reference,
            SecretEdit::Remove => None,
            SecretEdit::Replace(_) => Some(
                DatasourceSecretRef::new(
                    request.write.scope.workspace_id,
                    profile_id,
                    old_reference.map_or(1, |reference| reference.generation().saturating_add(1)),
                )
                .map_err(|_| error(DsErrorCode::InvalidField))?,
            ),
        };
        let revision = DatasourceRevision::new(DatasourceRevisionInput {
            schema_version: 1,
            workspace_id: request.write.scope.workspace_id,
            profile_id,
            revision: revision_number,
            adapter_id: request.adapter_id,
            adapter_version: request.adapter_version,
            config_version: request.config_version,
            source_id: source_id.clone(),
            fields: request.fields,
            context: request.context,
            credential: new_reference,
        })
        .map_err(|_| error(DsErrorCode::InvalidField))?;
        let profile = DatasourceProfile {
            schema_version: 1,
            workspace_id: request.write.scope.workspace_id,
            profile_id,
            source_id,
            name: request.name,
            head_revision: NonZeroU64::new(revision_number)
                .ok_or_else(|| error(DsErrorCode::InvalidField))?,
            deleted_at: None,
        };
        let bare_change = DatasourceChange::SaveRevision {
            profile: profile.clone(),
            revision: revision.clone(),
            mutation_id: None,
        };
        let command_digest =
            DatasourceDigest::of(&bare_change).map_err(|_| error(DsErrorCode::InvalidField))?;
        let mut mutation = (old_reference != new_reference).then(|| SecretMutation {
            schema_version: 1,
            mutation_id: OperationId::new(),
            write: request.write,
            profile_id,
            old: old_reference,
            new: new_reference,
            phase: SecretMutationPhase::Prepared,
            command_digest: command_digest.clone(),
        });
        if let Some(current) = mutation.as_mut() {
            self.repository
                .commit(DatasourceCommit {
                    schema_version: 1,
                    write: request.write,
                    command_digest: command_digest.clone(),
                    change: DatasourceChange::SecretJournal {
                        mutation: current.clone(),
                    },
                })
                .await?;
            if let SecretEdit::Replace(secret) = request.secret {
                if self.vault.protection().await? != ProtectionStatus::OwnerOnlyEncryptedFile {
                    self.rollback_uncommitted_mutation(current).await;
                    return Err(error(DsErrorCode::ProtectionUnavailable));
                }
                if let Err(error) = self
                    .vault
                    .write(current.new.expect("replacement"), secret)
                    .await
                {
                    self.rollback_uncommitted_mutation(current).await;
                    return Err(error);
                }
            }
            current.phase = SecretMutationPhase::VaultWritten;
            if let Err(error) = self
                .repository
                .commit(DatasourceCommit {
                    schema_version: 1,
                    write: request.write,
                    command_digest: command_digest.clone(),
                    change: DatasourceChange::SecretJournal {
                        mutation: current.clone(),
                    },
                })
                .await
            {
                self.rollback_uncommitted_mutation(current).await;
                return Err(error);
            }
        }
        let command = DatasourceCommit {
            schema_version: 1,
            write: request.write,
            command_digest,
            change: DatasourceChange::SaveRevision {
                profile,
                revision,
                mutation_id: mutation.as_ref().map(|mutation| mutation.mutation_id),
            },
        };
        let receipt = match self.repository.commit(command).await {
            Ok(receipt) => receipt,
            Err(error) => {
                if let Some(mutation) = &mutation {
                    self.rollback_uncommitted_mutation(mutation).await;
                }
                return Err(error);
            }
        };
        if let Some(mutation) = mutation.as_mut() {
            mutation.phase = SecretMutationPhase::Committed;
            self.cleanup_committed_mutation(mutation).await;
        }
        receipt
            .snapshot
            .profiles
            .into_iter()
            .find(|detail| detail.profile.profile_id == profile_id)
            .ok_or_else(|| error(DsErrorCode::Storage))
    }

    async fn validate(&self, request: ValidateDatasource) -> DsResult<ValidationReport> {
        if self.was_cancelled(request.operation_id) {
            return Err(operation_error(
                DsErrorCode::Cancelled,
                request.operation_id,
            ));
        }
        let detail = self.repository.load_revision(request.revision).await?;
        let factory = self.catalog.factory(
            &detail.revision.input().adapter_id,
            &detail.revision.input().adapter_version,
        )?;
        let mut fields = factory.validate_config(&detail.revision);
        let governance = self.policy.governance_for(&detail.revision);
        if governance.is_err() && fields.is_empty() {
            fields.push(FieldIssue {
                field: FieldId::new("database_path").expect("static field"),
                code: FieldIssueCode::Invalid,
            });
        }
        if let Some(reference) = detail.revision.input().credential
            && self.vault.read(reference).await.is_err()
            && fields.is_empty()
        {
            fields.push(FieldIssue {
                field: FieldId::new("password").expect("static field"),
                code: FieldIssueCode::Missing,
            });
        }

        let (state, evidence) = if !fields.is_empty() {
            (RevisionState::Invalid(DsErrorCode::InvalidField), None)
        } else if request.mode == ValidationMode::Local {
            (RevisionState::Draft, None)
        } else {
            match self.open_revision(&detail).await {
                Err(failure) => (RevisionState::Invalid(failure.code), None),
                Ok((connector, inputs, engine_version)) => {
                    let probe = connector.probe().await;
                    let close = connector.close().await;
                    match (probe, close) {
                        (Ok(probe), Ok(())) if !self.was_cancelled(request.operation_id) => {
                            let evidence =
                                ValidationEvidence::new(inputs, engine_version, probe, Utc::now())
                                    .map_err(|_| error(DsErrorCode::ReadOnlyUnproven))?;
                            (RevisionState::Ready, Some(evidence))
                        }
                        (Err(failure), _) | (_, Err(failure)) => {
                            (RevisionState::Invalid(failure.code), None)
                        }
                        _ => {
                            return Err(operation_error(
                                DsErrorCode::Cancelled,
                                request.operation_id,
                            ));
                        }
                    }
                }
            }
        };
        let change = DatasourceChange::Validation {
            revision: request.revision,
            state: state.clone(),
            evidence: evidence.clone(),
        };
        self.repository
            .commit(DatasourceCommit {
                schema_version: 1,
                write: request.write,
                command_digest: DatasourceDigest::of(&change)
                    .map_err(|_| error(DsErrorCode::InvalidField))?,
                change,
            })
            .await?;
        Ok(ValidationReport {
            schema_version: 1,
            revision: request.revision,
            mode: request.mode,
            fields,
            evidence,
            state,
        })
    }

    async fn select(&self, request: SelectDatasource) -> DsResult<SelectionSnapshot> {
        let detail = self.repository.load_revision(request.revision).await?;
        let descriptor = self.descriptor(
            &detail.revision.input().adapter_id,
            &detail.revision.input().adapter_version,
        )?;
        let governance = self.policy.governance_for(&detail.revision)?;
        let source = detail
            .revision
            .input()
            .source_id
            .as_ref()
            .ok_or_else(|| error(DsErrorCode::PolicyDenied))?;
        let capability = self.capability(&descriptor, source, &governance);
        let current_inputs = DatasourceValidationInputs::new(
            &detail.revision,
            &capability,
            governance.policy_digest.clone(),
        )
        .map_err(|_| error(DsErrorCode::ValidationStale))?;
        if !detail.is_ready(&current_inputs) {
            return Err(error(DsErrorCode::ValidationStale));
        }
        let (candidate, inputs, _) = self.open_revision(&detail).await?;
        if inputs != current_inputs {
            let _ = candidate.close().await;
            return Err(error(DsErrorCode::ValidationStale));
        }
        if let Err(error) = candidate.probe().await {
            let _ = candidate.close().await;
            return Err(error);
        }
        let change = DatasourceChange::Selection {
            revision: request.revision,
            kind: request.kind,
        };
        let committed = self
            .repository
            .commit(DatasourceCommit {
                schema_version: 1,
                write: request.write,
                command_digest: DatasourceDigest::of(&change)
                    .map_err(|_| error(DsErrorCode::InvalidField))?,
                change,
            })
            .await;
        let _ = candidate.close().await;
        Ok(committed?.snapshot.selection)
    }

    async fn delete(&self, request: DeleteDatasource) -> DsResult<SelectionSnapshot> {
        let change = DatasourceChange::Delete {
            profile_id: request.profile_id,
            disposition: request.disposition,
        };
        let receipt = self
            .repository
            .commit(DatasourceCommit {
                schema_version: 1,
                write: request.write,
                command_digest: DatasourceDigest::of(&change)
                    .map_err(|_| error(DsErrorCode::InvalidField))?,
                change,
            })
            .await?;
        Ok(receipt.snapshot.selection)
    }

    async fn doctor(&self, request: DatasourceDoctorRequest) -> DsResult<DatasourceDoctorReport> {
        if self.was_cancelled(request.operation_id) {
            return Err(operation_error(
                DsErrorCode::Cancelled,
                request.operation_id,
            ));
        }
        let revision = match request.revision {
            Some(revision) => revision,
            None => self
                .repository
                .load(request.scope)
                .await?
                .selection
                .current
                .ok_or_else(|| error(DsErrorCode::ValidationStale))?,
        };
        let detail = self.repository.load_revision(revision).await?;
        let result = self.open_revision(&detail).await;
        match result {
            Ok((connector, inputs, engine_version)) => {
                let probe = connector.probe().await;
                let _ = connector.close().await;
                match probe {
                    Ok(probe) => Ok(DatasourceDoctorReport {
                        schema_version: 1,
                        validation: Some(ValidationReport {
                            schema_version: 1,
                            revision,
                            mode: ValidationMode::Connection,
                            fields: Vec::new(),
                            evidence: ValidationEvidence::new(
                                inputs,
                                engine_version,
                                probe,
                                Utc::now(),
                            )
                            .ok(),
                            state: RevisionState::Ready,
                        }),
                        findings: Vec::new(),
                    }),
                    Err(error) => Ok(DatasourceDoctorReport {
                        schema_version: 1,
                        validation: None,
                        findings: vec![error],
                    }),
                }
            }
            Err(error) => Ok(DatasourceDoctorReport {
                schema_version: 1,
                validation: None,
                findings: vec![error],
            }),
        }
    }

    async fn cancel(&self, operation: OperationId) -> DsResult<()> {
        self.cancelled
            .lock()
            .map_err(|_| error(DsErrorCode::Storage))?
            .insert(operation);
        Ok(())
    }

    async fn receipt(&self, command: CommandId) -> DsResult<Option<DatasourceReceipt>> {
        self.repository.receipt(command).await
    }
}

fn error(code: DsErrorCode) -> DsError {
    DsError {
        code,
        field: None,
        remediation: match code {
            DsErrorCode::Conflict => DsRemediation::Refresh,
            DsErrorCode::ValidationStale => DsRemediation::Revalidate,
            DsErrorCode::PolicyDenied => DsRemediation::RepairPolicy,
            DsErrorCode::ProtectionUnavailable => DsRemediation::RepairProtection,
            DsErrorCode::InUse => DsRemediation::WaitOrCancelRun,
            _ => DsRemediation::EditConfiguration,
        },
        operation_id: None,
    }
}

fn field_error(code: DsErrorCode, field: FieldId) -> DsError {
    let mut error = error(code);
    error.field = Some(field);
    error
}

fn operation_error(code: DsErrorCode, operation: OperationId) -> DsError {
    let mut error = error(code);
    error.operation_id = Some(operation);
    error
}

fn replayed_save(
    receipt: DatasourceReceipt,
    request: &SaveDatasource,
) -> DsResult<DatasourceDetail> {
    let expected_committed_version = request
        .write
        .expected_version
        .checked_add(1)
        .ok_or_else(|| error(DsErrorCode::Conflict))?;
    if receipt.committed_version != expected_committed_version {
        return Err(error(DsErrorCode::Conflict));
    }
    let detail = receipt
        .snapshot
        .profiles
        .into_iter()
        .find(|detail| match request.profile_id {
            Some(profile_id) => detail.profile.profile_id == profile_id,
            None => detail.profile.name == request.name,
        })
        .ok_or_else(|| error(DsErrorCode::Conflict))?;
    let input = detail.revision.input();
    let expected_revision = request
        .write
        .expected_head_revision
        .map_or(1, |revision| revision.get().saturating_add(1));
    let secret_matches = match request.secret {
        SecretEdit::Keep => request.profile_id.is_some() || input.credential.is_none(),
        SecretEdit::Replace(_) => input.credential.is_some(),
        SecretEdit::Remove => input.credential.is_none(),
    };
    if detail.profile.name != request.name
        || detail.revision.number() != expected_revision
        || input.adapter_id != request.adapter_id
        || input.adapter_version != request.adapter_version
        || input.config_version != request.config_version
        || input.fields != request.fields
        || input.context != request.context
        || !secret_matches
    {
        return Err(error(DsErrorCode::Conflict));
    }
    Ok(detail)
}
