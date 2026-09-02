//! Provider Profile lifecycle orchestration.
//!
//! This module owns non-sensitive Profile browsing, Draft revision persistence, and copying. It
//! deliberately has no Vault, OAuth, model client, probe, Query, or deletion authority; those
//! flows arrive in their dedicated tasks.

use std::sync::Arc;

use ys_agent_core::{
    ActiveProviderSnapshot, CredentialGeneration, CredentialKind, CredentialMutation,
    CredentialMutationIntent, CredentialMutationOperation, CredentialMutationRepository,
    CredentialMutationRequest, CredentialProtectionStatus, CredentialVault, CredentialViewStatus,
    ProfileDetail, ProfileId, ProfileName, ProfileRevision, ProfileRevisionRepository,
    ProfileSummary, ProtectedCredentialWrite, ProviderCredentialReference, ProviderErrorCode,
    ProviderField, ProviderManagementError, ProviderRemediation, ProviderResult,
    RevisionPrecondition, RunProviderBindingRepository, SaveProfileRequest, SaveProfileRevision,
    SecretValue,
};
use zeroize::Zeroizing;

use super::validation::{LocalProfileValidationRequest, LocalProfileValidator};

/// The Profile-only portion of the Provider-management application service.
///
/// It depends on the narrow revision port so creating, listing, editing, and copying Profiles do
/// not acquire unintended credential mutation or deletion authority.
pub struct ProviderManagementService {
    profiles: Arc<dyn ProfileRevisionRepository>,
    local_validator: LocalProfileValidator,
}

/// Credential-only mutation orchestration.
///
/// The service owns no secret persistence itself. It records an immutable intent first, limits
/// plaintext use to one Vault call, and advances the visible Profile pointer only after the
/// protected generation is durable. Validation and activation deliberately remain separate.
pub struct CredentialService {
    profiles: Arc<dyn CredentialMutationRepository>,
    run_bindings: Arc<dyn RunProviderBindingRepository>,
    vault: Arc<dyn CredentialVault>,
}

impl CredentialService {
    pub fn new(
        profiles: Arc<dyn CredentialMutationRepository>,
        run_bindings: Arc<dyn RunProviderBindingRepository>,
        vault: Arc<dyn CredentialVault>,
    ) -> Self {
        Self {
            profiles,
            run_bindings,
            vault,
        }
    }

    /// Creates, replaces, or deletes one API-key generation. Every successful operation appends
    /// an unvalidated Draft; it never moves the active pointer or exposes secret material.
    pub async fn mutate(
        &self,
        request: CredentialMutationRequest,
    ) -> ProviderResult<ProfileDetail> {
        let current = self
            .profiles
            .load_current_revision(request.intent.profile_id())
            .await?;
        let staged = validate_credential_request(&current, &request.intent, &request.mutation)?;
        let operation_id = request.intent.operation_id();
        let old_generation = request.intent.old_generation();
        let rollback_generation = request.intent.rollback_generation();
        let replacement_generation = request.intent.new_generation();

        self.profiles
            .begin_credential_mutation(request.intent.clone())
            .await?;

        if !matches!(
            self.vault.protection_status().await,
            Ok(CredentialProtectionStatus::ConfirmedNative)
        ) {
            return Err(self.block_after_uncertain_state(operation_id).await);
        }

        let write_result = match request.mutation {
            CredentialMutation::Replace(write) => self.vault.write_generation(write).await,
            CredentialMutation::Delete => {
                self.copy_old_generation_to_rollback(current.credential_generation(), staged)
                    .await
            }
        };
        if let Err(error) = write_result {
            return Err(self
                .rollback_staged_generation(operation_id, staged, error)
                .await);
        }

        if self
            .profiles
            .record_credential_vault_write(operation_id)
            .await
            .is_err()
        {
            // A protected generation may now exist without an authoritative SQLite pointer.
            // Normalize the backend error and persist a fail-closed state rather than guessing.
            return Err(self.block_after_uncertain_state(operation_id).await);
        }

        let replacement = ProfileRevision::draft(
            current.profile_id(),
            current
                .revision()
                .checked_add(1)
                .ok_or_else(internal_error)?,
            current.provider(),
            current.model().clone(),
            current.parameters().clone(),
            replacement_generation,
        )
        .map_err(|_| internal_error())?;
        let commit = ys_agent_core::CredentialPointerCommit::new(
            operation_id,
            current.profile_id(),
            current.revision(),
            replacement,
        )
        .map_err(|_| internal_error())?;
        if let Err(error) = self.profiles.commit_credential_pointer(commit).await {
            return Err(self
                .rollback_staged_generation(operation_id, staged, error)
                .await);
        }

        if let Some(old_generation) = old_generation {
            match self.generation_is_referenced(old_generation).await {
                Ok(true) => {}
                Ok(false) => {
                    if self.retire_generation(old_generation).await.is_err() {
                        return Err(self.block_after_uncertain_state(operation_id).await);
                    }
                }
                Err(_) => {
                    return Err(self.block_after_uncertain_state(operation_id).await);
                }
            }
        }
        if let Some(rollback_generation) = rollback_generation
            && self
                .delete_rollback_generation(rollback_generation)
                .await
                .is_err()
        {
            return Err(self.block_after_uncertain_state(operation_id).await);
        }

        if self
            .profiles
            .complete_credential_mutation(operation_id)
            .await
            .is_err()
        {
            return Err(self.block_after_uncertain_state(operation_id).await);
        }
        self.load_profile(current.profile_id()).await
    }

    async fn load_profile(&self, profile_id: ProfileId) -> ProviderResult<ProfileDetail> {
        let summary = self
            .profiles
            .list_profiles()
            .await?
            .into_iter()
            .find(|summary| summary.profile_id == profile_id)
            .ok_or_else(profile_error)?;
        let revision = self.profiles.load_current_revision(profile_id).await?;
        Ok(profile_detail(summary, revision))
    }

    async fn copy_old_generation_to_rollback(
        &self,
        old_generation: Option<CredentialGeneration>,
        rollback_generation: CredentialGeneration,
    ) -> ProviderResult<()> {
        let old_generation = old_generation.ok_or_else(credential_stale_error)?;
        let lease = self
            .vault
            .read_generation(credential_reference(old_generation))
            .await?;
        let secret = lease.with_secret(|secret| {
            let mut copied = Zeroizing::new(secret.with_exposed(str::to_owned));
            SecretValue::from_utf8(std::mem::take(&mut *copied))
        });
        self.vault
            .write_generation(ProtectedCredentialWrite {
                reference: credential_reference(rollback_generation),
                secret,
            })
            .await
    }

    async fn generation_is_referenced(
        &self,
        generation: CredentialGeneration,
    ) -> ProviderResult<bool> {
        if self
            .run_bindings
            .has_nonterminal_credential_references(generation)
            .await?
        {
            return Ok(true);
        }
        let Some(active) = self.profiles.active().await? else {
            return Ok(false);
        };
        if active.profile_id() != generation.profile_id() {
            return Ok(false);
        }
        Ok(self
            .profiles
            .load_revision(active.profile_id(), active.profile_revision())
            .await?
            .credential_generation()
            == Some(generation))
    }

    async fn retire_generation(&self, generation: CredentialGeneration) -> ProviderResult<()> {
        self.vault
            .delete_generation(credential_reference(generation))
            .await?;
        self.profiles.retire_credential_generation(generation).await
    }

    async fn delete_rollback_generation(
        &self,
        generation: CredentialGeneration,
    ) -> ProviderResult<()> {
        self.vault
            .delete_generation(credential_reference(generation))
            .await
    }

    async fn rollback_staged_generation(
        &self,
        operation_id: ys_agent_core::OperationId,
        staged: CredentialGeneration,
        original: ProviderManagementError,
    ) -> ProviderManagementError {
        if self
            .vault
            .delete_generation(credential_reference(staged))
            .await
            .is_err()
        {
            return self.block_after_uncertain_state(operation_id).await;
        }
        match self
            .profiles
            .rollback_credential_mutation(operation_id)
            .await
        {
            Ok(_) => original,
            Err(_) => self.block_after_uncertain_state(operation_id).await,
        }
    }

    async fn block_after_uncertain_state(
        &self,
        operation_id: ys_agent_core::OperationId,
    ) -> ProviderManagementError {
        match self
            .profiles
            .block_credential_mutation(
                operation_id,
                ProviderErrorCode::CredentialProtectionUnavailable,
            )
            .await
        {
            Ok(_) => credential_protection_error(),
            Err(error) => error,
        }
    }
}

impl ProviderManagementService {
    pub fn new(profiles: Arc<dyn ProfileRevisionRepository>) -> Self {
        Self {
            profiles,
            local_validator: LocalProfileValidator::default(),
        }
    }

    /// Returns masked persisted summaries without any network, Vault, or model-client access.
    pub async fn list_profiles(&self) -> ProviderResult<Vec<ProfileSummary>> {
        self.profiles.list_profiles().await
    }

    /// Reads the immutable revision selected by the durable current pointer, including after a
    /// process restart. It never substitutes a historical or active revision.
    pub async fn load_profile(&self, profile_id: ProfileId) -> ProviderResult<ProfileDetail> {
        let summary = self.profile_summary(profile_id).await?;
        let revision = self.profiles.load_current_revision(profile_id).await?;
        Ok(profile_detail(summary, revision))
    }

    /// Persists the caller's next Draft revision under the repository CAS precondition. A failed
    /// save returns before this service can alter the active snapshot or any prior revision.
    pub async fn save_profile(&self, request: SaveProfileRequest) -> ProviderResult<ProfileDetail> {
        let profile_id = request.revision.revision.profile_id();
        self.validate_draft(&request.revision.name, &request.revision.revision)
            .await?;
        self.profiles.save_revision(request.revision).await?;
        self.load_profile(profile_id).await
    }

    /// Copies only non-sensitive configuration into a new first Draft. Credentials and validation
    /// are intentionally omitted so the new Profile cannot become active without its own setup
    /// and compatibility gate.
    pub async fn copy_profile(
        &self,
        source: ProfileId,
        name: ProfileName,
    ) -> ProviderResult<ProfileDetail> {
        let source_revision = self.profiles.load_current_revision(source).await?;
        let profile_id = ProfileId::new();
        let revision = ProfileRevision::draft(
            profile_id,
            1,
            source_revision.provider(),
            source_revision.model().clone(),
            source_revision.parameters().clone(),
            None,
        )
        .map_err(|_| profile_error())?;
        self.validate_copy_name(&name).await?;
        self.profiles
            .save_revision(SaveProfileRevision {
                precondition: RevisionPrecondition {
                    profile_id,
                    expected_current_revision: None,
                },
                name,
                revision,
            })
            .await?;
        self.load_profile(profile_id).await
    }

    /// Returns the durable active snapshot for offline browsing. `None` is the explicit no-active
    /// management state, not an invitation to choose another Profile.
    pub async fn active_snapshot(&self) -> ProviderResult<Option<ActiveProviderSnapshot>> {
        self.profiles.active().await
    }

    async fn profile_summary(&self, profile_id: ProfileId) -> ProviderResult<ProfileSummary> {
        self.profiles
            .list_profiles()
            .await?
            .into_iter()
            .find(|summary| summary.profile_id == profile_id)
            .ok_or_else(profile_error)
    }

    async fn validate_draft(
        &self,
        name: &ProfileName,
        revision: &ProfileRevision,
    ) -> ProviderResult<()> {
        let existing_profiles = self.profiles.list_profiles().await?;
        let credential_status = existing_profiles
            .iter()
            .find(|summary| summary.profile_id == revision.profile_id())
            .map(|summary| summary.credential_status)
            .unwrap_or(CredentialViewStatus::Missing);
        let existing_names = existing_profiles
            .iter()
            .map(|summary| {
                ProfileName::new(summary.name.clone())
                    .map(|name| (summary.profile_id, name))
                    .map_err(|_| profile_error())
            })
            .collect::<ProviderResult<Vec<_>>>()?;
        let validation = self
            .local_validator
            .validate_local(LocalProfileValidationRequest {
                profile_id: revision.profile_id(),
                name,
                provider: revision.provider(),
                model: revision.model(),
                parameters: revision.parameters(),
                credential_status,
                credential_generation: revision.credential_generation(),
                existing_names: &existing_names,
            });
        if validation.is_valid()
            || validation.violations().iter().all(|violation| {
                violation.error().code() == ProviderErrorCode::CredentialMissing.as_str()
            })
        {
            return Ok(());
        }
        Err(validation
            .violations()
            .iter()
            .find(|violation| {
                violation.error().code() != ProviderErrorCode::CredentialMissing.as_str()
            })
            .expect("non-credential local validation failure has a violation")
            .error()
            .clone())
    }

    async fn validate_copy_name(&self, name: &ProfileName) -> ProviderResult<()> {
        if self
            .profiles
            .list_profiles()
            .await?
            .iter()
            .any(|summary| summary.name == name.as_str())
        {
            return Err(ProviderManagementError::new(
                ProviderErrorCode::ProfileNameConflict,
                Some(ProviderField::ProfileName),
                ProviderRemediation::ReturnToEdit,
            ));
        }
        Ok(())
    }
}

fn profile_detail(summary: ProfileSummary, revision: ProfileRevision) -> ProfileDetail {
    ProfileDetail {
        summary,
        revision: revision.revision(),
        credential_generation: revision.credential_generation(),
        model: revision.model().clone(),
        parameters: revision.parameters().clone(),
        validation_id: revision.validation().map(|evidence| evidence.id()),
        // OAuth status is assembled by the OAuth lifecycle task. It is never inferred from a
        // credential pointer here.
        oauth_status: None,
    }
}

fn profile_error() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::StorageConflict,
        Some(ProviderField::Provider),
        ProviderRemediation::ReturnToEdit,
    )
}

fn validate_credential_request(
    current: &ProfileRevision,
    intent: &CredentialMutationIntent,
    mutation: &CredentialMutation,
) -> ProviderResult<CredentialGeneration> {
    if current.provider().required_credential_kind() != CredentialKind::ApiKey {
        return Err(ProviderManagementError::new(
            ProviderErrorCode::OAuthNotConnected,
            Some(ProviderField::OAuth),
            ProviderRemediation::Reauthorize,
        ));
    }
    if intent.profile_id() != current.profile_id()
        || intent.expected_revision() != current.revision()
        || intent.expected_generation() != current.credential_generation()
    {
        return Err(credential_stale_error());
    }

    match mutation {
        CredentialMutation::Replace(write) => {
            let expected_operation = if current.credential_generation().is_some() {
                CredentialMutationOperation::Replace
            } else {
                CredentialMutationOperation::Create
            };
            let Some(next_generation) = intent.new_generation() else {
                return Err(credential_stale_error());
            };
            if intent.operation() != expected_operation
                || write.reference != credential_reference(next_generation)
                || next_generation.kind() != CredentialKind::ApiKey
            {
                return Err(credential_stale_error());
            }
            Ok(next_generation)
        }
        CredentialMutation::Delete => {
            let Some(rollback_generation) = intent.rollback_generation() else {
                return Err(credential_stale_error());
            };
            if current.credential_generation().is_none()
                || intent.operation() != CredentialMutationOperation::Delete
                || intent.new_generation().is_some()
                || rollback_generation.kind() != CredentialKind::ApiKey
            {
                return Err(credential_stale_error());
            }
            Ok(rollback_generation)
        }
    }
}

fn credential_reference(generation: CredentialGeneration) -> ProviderCredentialReference {
    ProviderCredentialReference {
        profile_id: generation.profile_id(),
        generation,
    }
}

fn credential_stale_error() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::OperationStale,
        Some(ProviderField::Credential),
        ProviderRemediation::ReturnToEdit,
    )
}

fn credential_protection_error() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::CredentialProtectionUnavailable,
        Some(ProviderField::Credential),
        ProviderRemediation::ConfigureCredentialStore,
    )
}

fn internal_error() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::Internal,
        Some(ProviderField::Credential),
        ProviderRemediation::ContactSupport,
    )
}
