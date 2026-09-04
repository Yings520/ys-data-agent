use std::{fmt, sync::Arc};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use ys_agent_core::{
    ActiveProviderSnapshot, ActiveProviderView, CoreResult, CredentialProtectionStatus,
    CredentialVault, CredentialViewStatus, ProfileRevision, ProfileState,
    ProviderCredentialReference, ProviderDoctorView, ProviderErrorCode, ProviderField,
    ProviderManagementError, ProviderProfileRepository, ProviderRemediation, ProviderResult,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryCapability {
    GovernedMetric,
    AdHocRead,
    Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelReadiness {
    pub reachable: bool,
    pub supports_tool_calls: bool,
    pub supports_tool_call_ids: bool,
    pub supports_multi_turn_tool_results: bool,
    pub context_limit: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceReadiness {
    pub reachable: bool,
    pub query_capability: bool,
    pub catalog_capability: bool,
    pub freshness_capability: bool,
    pub database_read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorInputs {
    pub missing_config_keys: Vec<String>,
    pub model: ModelReadiness,
    pub source: SourceReadiness,
    pub query_policy_valid: bool,
    pub metric_registry_valid: bool,
    pub dbt_manifest_valid: Option<bool>,
    pub timezone_explicit: bool,
    pub freshness_rules_explicit: bool,
    pub query_budget_explicit: bool,
    pub artifact_directory_private_and_writable: bool,
    pub export_directory_private_and_writable: bool,
}

#[async_trait]
pub trait DoctorProbe: Send + Sync {
    async fn inspect(&self) -> CoreResult<DoctorInputs>;
}

#[async_trait]
pub trait DoctorRunner: Send + Sync {
    async fn run(&self) -> CoreResult<DoctorReport>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorReport {
    pub blocker_codes: Vec<String>,
    pub warning_codes: Vec<String>,
    pub ready_capabilities: Vec<QueryCapability>,
    pub repairs: Vec<String>,
}

impl DoctorReport {
    pub fn has_blocker(&self, code: &str) -> bool {
        self.blocker_codes.iter().any(|item| item == code)
    }

    pub fn has_warning(&self, code: &str) -> bool {
        self.warning_codes.iter().any(|item| item == code)
    }

    pub fn allows_query_submission(&self) -> bool {
        self.blocker_codes.is_empty() && !self.ready_capabilities.is_empty()
    }
}

/// Offline Provider readiness inspection. It reads only the committed active snapshot, its exact
/// immutable revision, masked Vault state, and credential journal metadata; it never opens a
/// credential lease, contacts a Provider, or runs a business Query.
pub struct ProviderDoctorCheck {
    profiles: Arc<dyn ProviderProfileRepository>,
    vault: Arc<dyn CredentialVault>,
}

impl ProviderDoctorCheck {
    pub fn new(
        profiles: Arc<dyn ProviderProfileRepository>,
        vault: Arc<dyn CredentialVault>,
    ) -> Self {
        Self { profiles, vault }
    }

    pub async fn run(&self) -> ProviderResult<ProviderDoctorView> {
        let Some(active) = self.profiles.active().await? else {
            return Ok(Self::no_active_view());
        };
        let revision = self
            .profiles
            .load_revision(active.profile_id(), active.profile_revision())
            .await?;
        let pending = self.profiles.pending_credential_mutations().await?;
        let mut port_blockers = Vec::new();
        let protection = match self.vault.protection_status().await {
            Ok(status) => Some(status),
            Err(error) => {
                port_blockers.push(error);
                None
            }
        };
        let credential_status = match revision.credential_generation() {
            Some(generation) => match self
                .vault
                .credential_status(ProviderCredentialReference {
                    profile_id: active.profile_id(),
                    generation,
                })
                .await
            {
                Ok(status) => Some(status),
                Err(error) => {
                    port_blockers.push(error);
                    None
                }
            },
            None => None,
        };

        let mut view = Self::evaluate(
            active,
            &revision,
            credential_status,
            protection,
            pending
                .iter()
                .filter(|record| record.profile_id() == revision.profile_id()),
        );
        view.blockers.extend(port_blockers);
        Ok(view)
    }

    pub fn no_active_view() -> ProviderDoctorView {
        ProviderDoctorView {
            active: None,
            credential_status: None,
            blockers: vec![provider_error(
                ProviderErrorCode::NoActiveProfile,
                ProviderField::Activation,
                ProviderRemediation::ActivateAnotherProfile,
            )],
            warnings: Vec::new(),
        }
    }

    fn evaluate<'a>(
        active: ActiveProviderSnapshot,
        revision: &ProfileRevision,
        credential_status: Option<CredentialViewStatus>,
        protection: Option<CredentialProtectionStatus>,
        pending: impl Iterator<Item = &'a ys_agent_core::CredentialMutationRecord>,
    ) -> ProviderDoctorView {
        let mut blockers = Vec::new();
        let mut warnings = Vec::new();
        let matching_revision = revision.profile_id() == active.profile_id()
            && revision.revision() == active.profile_revision();
        let validation_is_fresh = revision.validation().is_some_and(|evidence| {
            evidence.passed()
                && evidence.id() == active.validation_id()
                && evidence.digest() == *active.validation_digest()
        });
        if !matching_revision
            || revision.state() != ProfileState::Ready
            || !validation_is_fresh
            || revision.credential_generation() != Some(active.credential_generation())
        {
            // A passing validation is the persisted, non-network proof that this exact model
            // completed the tool-call, non-empty-ID, multi-turn-result, and known-context probe.
            blockers.push(provider_error(
                ProviderErrorCode::ValidationStale,
                ProviderField::Validation,
                ProviderRemediation::ValidateProfile,
            ));
        }

        match credential_status {
            Some(CredentialViewStatus::Saved) => {}
            Some(CredentialViewStatus::Missing) | None => blockers.push(provider_error(
                ProviderErrorCode::CredentialMissing,
                ProviderField::Credential,
                ProviderRemediation::ConfigureCredentialStore,
            )),
            Some(CredentialViewStatus::Expired | CredentialViewStatus::Revoked) => {
                let remediation =
                    if revision.provider() == ys_agent_core::ProviderId::ChatGptSubscription {
                        ProviderRemediation::Reauthorize
                    } else {
                        ProviderRemediation::ReturnToEdit
                    };
                blockers.push(provider_error(
                    ProviderErrorCode::AuthenticationInvalid,
                    ProviderField::Credential,
                    remediation,
                ));
            }
            Some(CredentialViewStatus::ProtectionUnavailable) => blockers.push(provider_error(
                ProviderErrorCode::CredentialProtectionUnavailable,
                ProviderField::Credential,
                ProviderRemediation::ConfigureCredentialStore,
            )),
            Some(CredentialViewStatus::ReconciliationRequired) => blockers.push(provider_error(
                ProviderErrorCode::CredentialProtectionUnavailable,
                ProviderField::Credential,
                ProviderRemediation::ContactSupport,
            )),
        }

        if !protection.is_some_and(CredentialProtectionStatus::is_confirmed) {
            blockers.push(provider_error(
                ProviderErrorCode::CredentialProtectionUnavailable,
                ProviderField::Credential,
                ProviderRemediation::ConfigureCredentialStore,
            ));
        }

        for record in pending {
            if record.blocks_profile_use() {
                blockers.push(provider_error(
                    record
                        .error_code()
                        .unwrap_or(ProviderErrorCode::CredentialProtectionUnavailable),
                    ProviderField::Credential,
                    ProviderRemediation::ContactSupport,
                ));
            } else if record.requires_reconciliation() {
                warnings.push(provider_error(
                    ProviderErrorCode::OperationStale,
                    ProviderField::Credential,
                    ProviderRemediation::WaitForCurrentOperation,
                ));
            }
        }

        ProviderDoctorView {
            active: Some(ActiveProviderView::from(&active)),
            credential_status,
            blockers,
            warnings,
        }
    }
}

fn provider_error(
    code: ProviderErrorCode,
    field: ProviderField,
    remediation: ProviderRemediation,
) -> ProviderManagementError {
    ProviderManagementError::new(code, Some(field), remediation)
}

impl fmt::Display for DoctorReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(formatter, "Workspace Doctor")?;
        writeln!(formatter, "blockers: {}", self.blocker_codes.join(", "))?;
        writeln!(formatter, "warnings: {}", self.warning_codes.join(", "))?;
        for repair in &self.repairs {
            writeln!(formatter, "repair: {repair}")?;
        }
        Ok(())
    }
}

pub struct WorkspaceDoctor {
    probe: Arc<dyn DoctorProbe>,
}

impl WorkspaceDoctor {
    pub fn new(probe: Arc<dyn DoctorProbe>) -> Self {
        Self { probe }
    }

    fn evaluate(inputs: DoctorInputs) -> DoctorReport {
        let mut blockers = Vec::new();
        let mut warnings = Vec::new();
        let mut repairs = Vec::new();
        let mut capabilities = vec![
            QueryCapability::GovernedMetric,
            QueryCapability::AdHocRead,
            QueryCapability::Metadata,
        ];

        if !inputs.missing_config_keys.is_empty() {
            blockers.push("required_config_missing".to_owned());
            repairs.push(format!(
                "Define required configuration keys (using CredentialReference for secrets) for: {}",
                inputs.missing_config_keys.join(", ")
            ));
        }

        if !inputs.model.reachable
            || !inputs.model.supports_tool_calls
            || !inputs.model.supports_tool_call_ids
            || !inputs.model.supports_multi_turn_tool_results
            || inputs.model.context_limit.is_none()
        {
            blockers.push("model_protocol_incompatible".to_owned());
            repairs.push(
                "Configure and validate a reachable Provider Profile with tool calls, tool call IDs, multi-turn tool results, and a known context limit"
                    .to_owned(),
            );
        }

        if !inputs.source.reachable || !inputs.source.query_capability {
            blockers.push("connector_unavailable".to_owned());
            repairs.push(
                "Repair the configured Query connector and verify its capabilities".to_owned(),
            );
        }

        if !inputs.source.database_read_only {
            blockers.push("database_not_read_only".to_owned());
            repairs.push(
                "Use a database-enforced read-only role or SQLite read-only flags plus PRAGMA query_only = 1"
                    .to_owned(),
            );
        }

        if !inputs.query_policy_valid {
            blockers.push("query_policy_invalid".to_owned());
            repairs.push("Repair the Query Policy before submitting a query".to_owned());
        }

        if !inputs.metric_registry_valid {
            warnings.push("metric_registry_missing".to_owned());
            repairs.push("Provide a valid Metric Registry to enable GovernedMetric".to_owned());
            capabilities.retain(|item| *item != QueryCapability::GovernedMetric);
        }

        match inputs.dbt_manifest_valid {
            None => {
                warnings.push("dbt_manifest_missing".to_owned());
                repairs.push(
                    "Configure the optional dbt manifest to improve engineering Context".to_owned(),
                );
            }
            Some(false) => {
                warnings.push("dbt_manifest_invalid".to_owned());
                repairs.push("Repair or remove the optional dbt manifest path".to_owned());
            }
            Some(true) => {}
        }

        if !inputs.source.freshness_capability {
            warnings.push("freshness_capability_missing".to_owned());
            repairs
                .push("Configure a Freshness reader for current/latest/SLA questions".to_owned());
        }

        if !inputs.timezone_explicit {
            blockers.push("timezone_missing".to_owned());
            repairs.push("Set an explicit Workspace timezone".to_owned());
        }

        if !inputs.freshness_rules_explicit {
            warnings.push("freshness_rules_missing".to_owned());
            repairs.push("Define Freshness rules for time-sensitive answers".to_owned());
        }

        if !inputs.query_budget_explicit {
            blockers.push("query_budget_missing".to_owned());
            repairs.push("Set timeout, row, byte, and optional cost limits".to_owned());
        }

        if !inputs.artifact_directory_private_and_writable {
            blockers.push("artifact_directory_unsafe".to_owned());
            repairs.push("Make .ysda/artifacts owner-only and writable".to_owned());
        }

        if !inputs.export_directory_private_and_writable {
            warnings.push("export_directory_unsafe".to_owned());
            repairs.push("Make .ysda/exports owner-only and writable before exporting".to_owned());
        }

        if !inputs.source.catalog_capability {
            capabilities.retain(|item| *item != QueryCapability::Metadata);
        }
        if !blockers.is_empty() {
            capabilities.clear();
        }

        DoctorReport {
            blocker_codes: blockers,
            warning_codes: warnings,
            ready_capabilities: capabilities,
            repairs,
        }
    }
}

#[async_trait]
impl DoctorRunner for WorkspaceDoctor {
    async fn run(&self) -> CoreResult<DoctorReport> {
        let inputs = self.probe.inspect().await?;
        Ok(Self::evaluate(inputs))
    }
}

#[cfg(test)]
mod provider_doctor_tests {
    use super::*;
    use ys_agent_core::{
        CompatibilityEvidence, CredentialGeneration, CredentialKind, CredentialMutationIntent,
        CredentialMutationPhase, CredentialMutationRecord, OperationId, ProfileId, ProviderId,
        ProviderModelId, ProviderParameters, ValidationVersions,
    };

    fn ready_revision(profile_id: ProfileId) -> ProfileRevision {
        let generation = CredentialGeneration::new(profile_id, 1, CredentialKind::ApiKey)
            .expect("valid credential generation");
        let mut revision = ProfileRevision::draft(
            profile_id,
            1,
            ProviderId::DeepSeek,
            ProviderModelId::new(ProviderId::DeepSeek, "deepseek/doctor-test")
                .expect("valid test model"),
            ProviderParameters::default(),
            Some(generation),
        )
        .expect("valid draft");
        let versions = ValidationVersions::new("catalog", "probe", "liter", "codec");
        let evidence = CompatibilityEvidence::passing(revision.validation_inputs(versions.clone()));
        revision
            .accept_validation(evidence, versions)
            .expect("ready revision");
        revision
    }

    fn evaluate(
        revision: &ProfileRevision,
        credential_status: CredentialViewStatus,
        pending: impl Iterator<Item = CredentialMutationRecord>,
    ) -> ProviderDoctorView {
        let active = ActiveProviderSnapshot::from_ready(revision, 1).expect("active snapshot");
        let pending = pending.collect::<Vec<_>>();
        ProviderDoctorCheck::evaluate(
            active,
            revision,
            Some(credential_status),
            Some(CredentialProtectionStatus::ConfirmedNative),
            pending.iter(),
        )
    }

    #[test]
    fn no_active_provider_is_a_stable_doctor_blocker() {
        let view = ProviderDoctorCheck::no_active_view();

        assert!(
            view.blockers
                .iter()
                .any(|blocker| blocker.code() == "provider.no_active_profile")
        );
    }

    #[test]
    fn ready_active_revision_has_only_masked_ready_status() {
        let revision = ready_revision(ProfileId::new());
        let view = evaluate(&revision, CredentialViewStatus::Saved, std::iter::empty());

        assert!(view.blockers.is_empty());
        assert!(view.warnings.is_empty());
        assert!(view.active.is_some());
        assert_eq!(view.credential_status, Some(CredentialViewStatus::Saved));
        assert!(!format!("{view:?}").contains("doctor-test-secret"));
    }

    #[test]
    fn stale_revision_and_unavailable_credentials_are_actionable_blockers() {
        let ready = ready_revision(ProfileId::new());
        let active = ActiveProviderSnapshot::from_ready(&ready, 1).expect("active snapshot");
        let stale = ProfileRevision::draft(
            ready.profile_id(),
            ready.revision(),
            ready.provider(),
            ready.model().clone(),
            ready.parameters().clone(),
            ready.credential_generation(),
        )
        .expect("stale revision fixture");
        let view = ProviderDoctorCheck::evaluate(
            active,
            &stale,
            Some(CredentialViewStatus::Missing),
            Some(CredentialProtectionStatus::ConfirmedNative),
            std::iter::empty(),
        );

        assert!(
            view.blockers
                .iter()
                .any(|blocker| blocker.code() == "provider.validation.stale")
        );
        assert!(
            view.blockers
                .iter()
                .any(|blocker| blocker.code() == "provider.credential.missing")
        );
    }

    #[test]
    fn expired_credential_and_unconfirmed_vault_fail_closed() {
        let revision = ready_revision(ProfileId::new());
        let active = ActiveProviderSnapshot::from_ready(&revision, 1).expect("active snapshot");
        let view = ProviderDoctorCheck::evaluate(
            active,
            &revision,
            Some(CredentialViewStatus::Expired),
            Some(CredentialProtectionStatus::Unconfirmed),
            std::iter::empty(),
        );

        assert!(
            view.blockers
                .iter()
                .any(|blocker| blocker.code() == "provider.auth.invalid")
        );
        assert!(view.blockers.iter().any(|blocker| {
            blocker.code() == "provider.credential.protection_unavailable"
                && blocker.remediation() == ProviderRemediation::ConfigureCredentialStore
        }));
    }

    #[test]
    fn reconciliation_state_never_becomes_ready_and_blocked_state_is_reported() {
        let revision = ready_revision(ProfileId::new());
        let generation = revision.credential_generation().expect("ready generation");
        let intent = CredentialMutationIntent::create(
            OperationId::new(),
            revision.profile_id(),
            revision.revision(),
            generation,
        )
        .expect("valid journal intent");
        let pending = CredentialMutationRecord::intent_recorded(intent);
        let warning = evaluate(
            &revision,
            CredentialViewStatus::Saved,
            std::iter::once(pending),
        );
        assert!(
            warning
                .warnings
                .iter()
                .any(|warning| warning.code() == "provider.operation.stale")
        );

        let blocked = CredentialMutationRecord::intent_recorded(
            CredentialMutationIntent::create(
                OperationId::new(),
                revision.profile_id(),
                revision.revision(),
                generation,
            )
            .expect("valid blocked journal intent"),
        )
        .transition(
            CredentialMutationPhase::Blocked,
            Some(ProviderErrorCode::CredentialProtectionUnavailable),
        )
        .expect("blocked transition");
        let blocked = evaluate(
            &revision,
            CredentialViewStatus::ReconciliationRequired,
            std::iter::once(blocked),
        );
        assert!(
            blocked
                .blockers
                .iter()
                .any(|blocker| blocker.code() == "provider.credential.protection_unavailable")
        );
    }
}
