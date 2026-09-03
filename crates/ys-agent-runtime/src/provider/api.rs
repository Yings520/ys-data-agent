//! In-process implementation of the masked Provider-management application boundary.
//!
//! This facade is the only place that composes Profile lifecycle, Vault, discovery, compatibility
//! probing, and Doctor. Callers receive core view types and stable `ProviderManagementError`s;
//! neither a repository nor a credential lease escapes it.

use std::{collections::HashSet, sync::Arc};

use async_trait::async_trait;
use ys_agent_core::{
    ActivateProfileRequest, ActivationPrecondition, ActiveProviderView, CompatibilityEvidenceView,
    CredentialGeneration, CredentialMutationRequest, CredentialVault, CredentialViewStatus,
    DeleteProfileRequest, DeviceAuthorizationView, DiscoverModelsRequest, DiscoveredModel,
    ListModelCandidatesRequest, ModelCandidateBatch, ModelCandidateKey, ModelCandidateStatus,
    ModelCandidateView, ModelDiscovery, ModelSelectionSnapshot, OAuthConnectionView, OperationId,
    ProfileDetail, ProfileId, ProfileName, ProfileRevision, ProfileState, ProviderCatalogView,
    ProviderClientBinding, ProviderClientFactory, ProviderCredentialReference, ProviderDoctorView,
    ProviderErrorCode, ProviderField, ProviderManagementApi, ProviderManagementError,
    ProviderModelId, ProviderProfileRepository, ProviderRemediation, ProviderResult,
    ProviderSupportStatus, RunProviderBindingRepository, SelectionAvailability,
    SelectionCurrentStatus, SelectionTargetView, ValidateProfileRequest, ValidationCommit,
    ValidationCommitPrecondition,
};

use super::{
    catalog::GovernedProviderCatalog,
    service::{CredentialService, ProviderManagementService},
    validation::{
        CompatibilityProbeRequest, CompatibilityValidator, LocalProfileValidationRequest,
        LocalProfileValidator, ModelContextLimit,
    },
};
use crate::doctor::ProviderDoctorCheck;

/// Composes the existing Provider services behind the core `ProviderManagementApi` port.
///
/// Catalog views are injected as already-sanitized, offline data. Evidence collection decides
/// their support status elsewhere; this facade never calls an external registry while rendering.
pub struct InProcessProviderManagementApi {
    catalog: GovernedProviderCatalog,
    catalog_views: Vec<ProviderCatalogView>,
    profiles: Arc<dyn ProviderProfileRepository>,
    vault: Arc<dyn CredentialVault>,
    run_bindings: Arc<dyn RunProviderBindingRepository>,
    lifecycle: Arc<ProviderManagementService>,
    credentials: Arc<CredentialService>,
    discovery: Arc<dyn ModelDiscovery>,
    factory: Arc<dyn ProviderClientFactory>,
    local_validator: LocalProfileValidator,
    compatibility_validator: CompatibilityValidator,
    doctor: ProviderDoctorCheck,
}

impl InProcessProviderManagementApi {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        catalog: GovernedProviderCatalog,
        catalog_views: Vec<ProviderCatalogView>,
        profiles: Arc<dyn ProviderProfileRepository>,
        vault: Arc<dyn CredentialVault>,
        run_bindings: Arc<dyn RunProviderBindingRepository>,
        lifecycle: Arc<ProviderManagementService>,
        credentials: Arc<CredentialService>,
        discovery: Arc<dyn ModelDiscovery>,
        factory: Arc<dyn ProviderClientFactory>,
    ) -> Self {
        Self {
            catalog: catalog.clone(),
            catalog_views,
            doctor: ProviderDoctorCheck::new(profiles.clone(), vault.clone()),
            local_validator: LocalProfileValidator::new(catalog.clone()),
            compatibility_validator: CompatibilityValidator::new(catalog),
            profiles,
            vault,
            run_bindings,
            lifecycle,
            credentials,
            discovery,
            factory,
        }
    }

    async fn current_revision(
        &self,
        profile_id: ProfileId,
        expected_revision: u64,
    ) -> ProviderResult<ProfileRevision> {
        let revision = self.profiles.load_current_revision(profile_id).await?;
        if revision.revision() != expected_revision {
            return Err(stale_operation());
        }
        Ok(revision)
    }

    async fn generation_status(
        &self,
        revision: &ProfileRevision,
    ) -> ProviderResult<(CredentialGeneration, CredentialViewStatus)> {
        let generation = revision
            .credential_generation()
            .ok_or_else(credential_missing)?;
        let status = self
            .vault
            .credential_status(ProviderCredentialReference {
                profile_id: revision.profile_id(),
                generation,
            })
            .await?;
        Ok((generation, status))
    }

    async fn local_validation(
        &self,
        revision: &ProfileRevision,
        credential_status: CredentialViewStatus,
    ) -> ProviderResult<super::validation::LocalProfileValidation> {
        let detail = self.lifecycle.load_profile(revision.profile_id()).await?;
        let name = ProfileName::new(detail.summary.name).map_err(|_| internal_error())?;
        let profiles = self.profiles.list_profiles().await?;
        let names = profiles
            .iter()
            .map(|profile| {
                ProfileName::new(profile.name.clone())
                    .map(|name| (profile.profile_id, name))
                    .map_err(|_| internal_error())
            })
            .collect::<ProviderResult<Vec<_>>>()?;
        Ok(self
            .local_validator
            .validate_local(LocalProfileValidationRequest {
                profile_id: revision.profile_id(),
                name: &name,
                provider: revision.provider(),
                model: revision.model(),
                parameters: revision.parameters(),
                credential_status,
                credential_generation: revision.credential_generation(),
                existing_names: &names,
            }))
    }
}

#[async_trait]
impl ProviderManagementApi for InProcessProviderManagementApi {
    async fn catalog(&self) -> ProviderResult<Vec<ProviderCatalogView>> {
        Ok(self.catalog_views.clone())
    }

    async fn model_selection_snapshot(&self) -> ProviderResult<ModelSelectionSnapshot> {
        let profiles = self.lifecycle.list_profiles().await?;
        let active = self
            .lifecycle
            .active_snapshot()
            .await?
            .as_ref()
            .map(ActiveProviderView::from);
        let mut seen = HashSet::new();
        let mut targets = Vec::with_capacity(self.catalog_views.len());

        for view in &self.catalog_views {
            if !seen.insert(view.provider) {
                return Err(catalog_unavailable());
            }
            let current = if active
                .as_ref()
                .is_some_and(|active| active.provider == view.provider)
            {
                SelectionCurrentStatus::Current
            } else {
                SelectionCurrentStatus::NotCurrent
            };
            let configured = profiles
                .iter()
                .any(|profile| profile.provider == view.provider);
            let availability = match view.support_status {
                ProviderSupportStatus::Blocked => SelectionAvailability::Unavailable,
                ProviderSupportStatus::Supported | ProviderSupportStatus::Candidate
                    if configured =>
                {
                    SelectionAvailability::Configured
                }
                ProviderSupportStatus::Supported | ProviderSupportStatus::Candidate => {
                    SelectionAvailability::NeedsSetup
                }
            };
            targets.push(
                SelectionTargetView::new(
                    self.catalog.entry(view.provider).selection_target(),
                    view.display_name.clone(),
                    availability,
                    current,
                )
                .map_err(|_| catalog_unavailable())?,
            );
        }

        if active
            .as_ref()
            .is_some_and(|active| !seen.contains(&active.provider))
        {
            return Err(catalog_unavailable());
        }
        ModelSelectionSnapshot::new(targets).map_err(|_| catalog_unavailable())
    }

    async fn list_model_candidates(
        &self,
        request: ListModelCandidatesRequest,
    ) -> ProviderResult<ModelCandidateBatch> {
        let provider = request.target.provider();
        let mut catalog_views = self
            .catalog_views
            .iter()
            .filter(|view| view.provider == provider);
        let catalog_view = catalog_views.next().ok_or_else(catalog_unavailable)?;
        if catalog_views.next().is_some() {
            return Err(catalog_unavailable());
        }
        if self.catalog.entry(provider).selection_target() != request.target {
            return Err(invalid_selection_target());
        }

        let active = self
            .lifecycle
            .active_snapshot()
            .await?
            .as_ref()
            .map(ActiveProviderView::from);
        let expected_activation_revision = active.as_ref().map(|active| active.activation_revision);
        let summaries = self.lifecycle.list_profiles().await?;
        let mut candidates = Vec::new();

        for summary in summaries
            .into_iter()
            .filter(|summary| summary.provider == provider)
        {
            let revision = self
                .profiles
                .load_current_revision(summary.profile_id)
                .await?;
            if revision.provider() != provider {
                return Err(catalog_unavailable());
            }
            let credential_status = match revision.credential_generation() {
                Some(generation) => {
                    self.vault
                        .credential_status(ProviderCredentialReference {
                            profile_id: summary.profile_id,
                            generation,
                        })
                        .await?
                }
                None => CredentialViewStatus::Missing,
            };
            let current = candidate_current_status(active.as_ref(), &revision);
            let saved_status = candidate_status(
                summary.state,
                credential_status,
                &catalog_view.support_status,
                current,
            );
            candidates.push(model_candidate_view(
                summary.profile_id,
                revision.revision(),
                expected_activation_revision,
                provider,
                revision.model().clone(),
                &summary.name,
                saved_status,
                current,
            )?);

            if catalog_view.support_status == ProviderSupportStatus::Blocked
                || credential_status != CredentialViewStatus::Saved
            {
                continue;
            }
            let Some(credential_generation) = revision.credential_generation() else {
                continue;
            };
            let discovered = self
                .discover_models(DiscoverModelsRequest {
                    operation_id: OperationId::new(),
                    profile_id: summary.profile_id,
                    profile_revision: revision.revision(),
                    provider,
                    credential_generation,
                })
                .await;
            let Ok(discovered) = discovered else {
                continue;
            };
            let mut seen_models = HashSet::from([revision.model().as_str().to_owned()]);
            for discovered in discovered {
                let Ok(model) = ProviderModelId::new(provider, discovered.model) else {
                    continue;
                };
                if !seen_models.insert(model.as_str().to_owned()) {
                    continue;
                }
                candidates.push(model_candidate_view(
                    summary.profile_id,
                    revision.revision(),
                    expected_activation_revision,
                    provider,
                    model,
                    &summary.name,
                    ModelCandidateStatus::NeedsValidation,
                    SelectionCurrentStatus::NotCurrent,
                )?);
            }
        }

        ModelCandidateBatch::new(request.target, candidates).map_err(|_| catalog_unavailable())
    }

    async fn list_profiles(&self) -> ProviderResult<Vec<ys_agent_core::ProfileSummary>> {
        self.lifecycle.list_profiles().await
    }

    async fn active_provider(&self) -> ProviderResult<Option<ActiveProviderView>> {
        Ok(self
            .lifecycle
            .active_snapshot()
            .await?
            .as_ref()
            .map(ActiveProviderView::from))
    }

    async fn load_profile(&self, profile_id: ProfileId) -> ProviderResult<ProfileDetail> {
        self.lifecycle.load_profile(profile_id).await
    }

    async fn save_profile(
        &self,
        request: ys_agent_core::SaveProfileRequest,
    ) -> ProviderResult<ProfileDetail> {
        self.lifecycle.save_profile(request).await
    }

    async fn copy_profile(
        &self,
        source: ProfileId,
        name: ProfileName,
    ) -> ProviderResult<ProfileDetail> {
        self.lifecycle.copy_profile(source, name).await
    }

    async fn mutate_credential(
        &self,
        request: CredentialMutationRequest,
    ) -> ProviderResult<ProfileDetail> {
        self.credentials.mutate(request).await
    }

    async fn delete_profile(&self, request: DeleteProfileRequest) -> ProviderResult<()> {
        self.lifecycle
            .delete_profile(request, self.vault.as_ref(), self.run_bindings.as_ref())
            .await
    }

    async fn discover_models(
        &self,
        request: DiscoverModelsRequest,
    ) -> ProviderResult<Vec<DiscoveredModel>> {
        let revision = self
            .current_revision(request.profile_id, request.profile_revision)
            .await?;
        if revision.provider() != request.provider
            || revision.credential_generation() != Some(request.credential_generation)
        {
            return Err(stale_operation());
        }
        let (generation, status) = self.generation_status(&revision).await?;
        require_saved_credential(status)?;
        let credential = self
            .vault
            .read_generation(ProviderCredentialReference {
                profile_id: revision.profile_id(),
                generation,
            })
            .await?;
        self.discovery.discover(request, credential).await
    }

    async fn validate_profile(
        &self,
        request: ValidateProfileRequest,
    ) -> ProviderResult<CompatibilityEvidenceView> {
        let revision = self
            .current_revision(request.profile_id, request.revision)
            .await?;
        let (generation, credential_status) = self.generation_status(&revision).await?;
        let local_validation = self.local_validation(&revision, credential_status).await?;
        if let Some(violation) = local_validation.violations().first() {
            return Err(violation.error().clone());
        }
        let observed_context_limit = request
            .observed_context_limit
            .map(ModelContextLimit::from_directory)
            .ok_or_else(model_context_unknown)?;
        let oauth_status = if revision.provider() == ys_agent_core::ProviderId::ChatGptSubscription
        {
            Some(
                self.lifecycle
                    .oauth_connection(revision.profile_id())
                    .await?
                    .status,
            )
        } else {
            None
        };
        let credential = self
            .vault
            .read_generation(ProviderCredentialReference {
                profile_id: revision.profile_id(),
                generation,
            })
            .await?;
        let binding =
            ProviderClientBinding::from_revision(&revision).map_err(|_| internal_error())?;
        let client = self.factory.build(binding, credential).await?;
        let probe = self
            .compatibility_validator
            .probe_model(
                CompatibilityProbeRequest {
                    revision: &revision,
                    local_validation: &local_validation,
                    oauth_status,
                    observed_context_limit: Some(observed_context_limit),
                    codec_version: codec_version(revision.provider()),
                },
                client.as_ref(),
            )
            .await?;
        let evidence = probe.compatibility().clone();
        let detail = self
            .lifecycle
            .commit_validation(ValidationCommit {
                precondition: ValidationCommitPrecondition {
                    operation_id: request.operation_id,
                    profile_id: revision.profile_id(),
                    revision: revision.revision(),
                    credential_generation: generation,
                    validation_digest: evidence.digest(),
                },
                evidence,
                versions: probe.versions().clone(),
            })
            .await?;
        Ok(CompatibilityEvidenceView {
            validation_id: detail.validation_id.ok_or_else(internal_error)?,
            state: detail.summary.state,
            credential_status: detail.summary.credential_status,
            error: None,
        })
    }

    async fn activate(
        &self,
        request: ActivateProfileRequest,
    ) -> ProviderResult<ActiveProviderView> {
        self.lifecycle.activate(request).await
    }

    async fn activate_current(
        &self,
        profile_id: ProfileId,
        operation_id: OperationId,
    ) -> ProviderResult<ActiveProviderView> {
        let revision = self.profiles.load_current_revision(profile_id).await?;
        let validation = revision.validation().ok_or_else(model_context_unknown)?;
        let expected_activation_revision = self
            .profiles
            .active()
            .await?
            .map(|active| active.activation_revision());
        self.lifecycle
            .activate(ActivateProfileRequest {
                operation_id,
                precondition: ActivationPrecondition {
                    profile_id,
                    revision: revision.revision(),
                    validation_id: validation.id(),
                    validation_digest: validation.digest(),
                    expected_activation_revision,
                },
            })
            .await
    }

    async fn credential_status(
        &self,
        profile_id: ProfileId,
    ) -> ProviderResult<CredentialViewStatus> {
        let revision = self.profiles.load_current_revision(profile_id).await?;
        match revision.credential_generation() {
            Some(generation) => {
                self.vault
                    .credential_status(ProviderCredentialReference {
                        profile_id,
                        generation,
                    })
                    .await
            }
            None => Ok(CredentialViewStatus::Missing),
        }
    }

    async fn oauth_connection(&self, profile_id: ProfileId) -> ProviderResult<OAuthConnectionView> {
        self.lifecycle.oauth_connection(profile_id).await
    }

    async fn doctor(&self) -> ProviderResult<ProviderDoctorView> {
        self.doctor.run().await
    }

    async fn cancel_operation(&self, operation_id: OperationId) -> ProviderResult<()> {
        self.lifecycle.cancel_operation(operation_id)
    }

    async fn start_oauth(
        &self,
        profile_id: ProfileId,
        operation_id: OperationId,
    ) -> ProviderResult<DeviceAuthorizationView> {
        self.lifecycle.start_oauth(profile_id, operation_id).await
    }

    async fn complete_oauth(
        &self,
        operation_id: OperationId,
    ) -> ProviderResult<OAuthConnectionView> {
        self.lifecycle.complete_oauth(operation_id).await
    }

    async fn refresh_oauth(
        &self,
        profile_id: ProfileId,
        operation_id: OperationId,
    ) -> ProviderResult<OAuthConnectionView> {
        self.lifecycle.refresh_oauth(profile_id, operation_id).await
    }

    async fn reauthorize_oauth(
        &self,
        profile_id: ProfileId,
        operation_id: OperationId,
    ) -> ProviderResult<DeviceAuthorizationView> {
        self.lifecycle
            .reauthorize_oauth(profile_id, operation_id)
            .await
    }

    async fn logout_oauth(
        &self,
        profile_id: ProfileId,
        operation_id: OperationId,
    ) -> ProviderResult<ys_agent_core::RemoteRevocationOutcome> {
        self.lifecycle.logout_oauth(profile_id, operation_id).await
    }
}

fn require_saved_credential(status: CredentialViewStatus) -> ProviderResult<()> {
    match status {
        CredentialViewStatus::Saved => Ok(()),
        CredentialViewStatus::Missing => Err(credential_missing()),
        CredentialViewStatus::Expired | CredentialViewStatus::Revoked => {
            Err(ProviderManagementError::new(
                ProviderErrorCode::AuthenticationInvalid,
                Some(ProviderField::Credential),
                ProviderRemediation::ReturnToEdit,
            ))
        }
        CredentialViewStatus::ProtectionUnavailable
        | CredentialViewStatus::ReconciliationRequired => Err(ProviderManagementError::new(
            ProviderErrorCode::CredentialProtectionUnavailable,
            Some(ProviderField::Credential),
            ProviderRemediation::ConfigureCredentialStore,
        )),
    }
}

fn candidate_current_status(
    active: Option<&ActiveProviderView>,
    revision: &ProfileRevision,
) -> SelectionCurrentStatus {
    if active.is_some_and(|active| {
        active.profile_id == revision.profile_id()
            && active.profile_revision == revision.revision()
            && active.provider == revision.provider()
            && active.model == *revision.model()
    }) {
        SelectionCurrentStatus::Current
    } else {
        SelectionCurrentStatus::NotCurrent
    }
}

fn candidate_status(
    state: ProfileState,
    credential_status: CredentialViewStatus,
    support_status: &ProviderSupportStatus,
    current: SelectionCurrentStatus,
) -> ModelCandidateStatus {
    if current.is_current() {
        return ModelCandidateStatus::Ready;
    }
    if *support_status == ProviderSupportStatus::Blocked {
        return ModelCandidateStatus::CapabilityInsufficient;
    }
    if credential_status != CredentialViewStatus::Saved {
        return ModelCandidateStatus::Unavailable;
    }
    match state {
        ProfileState::Ready => ModelCandidateStatus::Ready,
        ProfileState::Draft => ModelCandidateStatus::NeedsValidation,
        ProfileState::Invalid => ModelCandidateStatus::ValidationExpired,
    }
}

#[allow(clippy::too_many_arguments)]
fn model_candidate_view(
    profile_id: ProfileId,
    profile_revision: u64,
    activation_revision: Option<u64>,
    provider: ys_agent_core::ProviderId,
    model: ProviderModelId,
    profile_name: &str,
    status: ModelCandidateStatus,
    current: SelectionCurrentStatus,
) -> ProviderResult<ModelCandidateView> {
    let model_display_name = model.as_str().to_owned();
    let key = ModelCandidateKey::new(
        profile_id,
        profile_revision,
        activation_revision,
        provider,
        model,
    )
    .map_err(|_| catalog_unavailable())?;
    ModelCandidateView::new(key, profile_name, model_display_name, status, current)
        .map_err(|_| catalog_unavailable())
}

fn invalid_selection_target() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::ModelIncompatible,
        Some(ProviderField::Provider),
        ProviderRemediation::ReturnToEdit,
    )
}

fn catalog_unavailable() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::ProtocolIncompatible,
        Some(ProviderField::Provider),
        ProviderRemediation::Retry,
    )
}

fn credential_missing() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::CredentialMissing,
        Some(ProviderField::Credential),
        ProviderRemediation::ConfigureCredentialStore,
    )
}

fn stale_operation() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::OperationStale,
        Some(ProviderField::Provider),
        ProviderRemediation::WaitForCurrentOperation,
    )
}

fn model_context_unknown() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::ModelIncompatible,
        Some(ProviderField::Model),
        ProviderRemediation::ValidateProfile,
    )
}

fn internal_error() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::Internal,
        None,
        ProviderRemediation::ContactSupport,
    )
}

fn codec_version(provider: ys_agent_core::ProviderId) -> &'static str {
    match provider {
        ys_agent_core::ProviderId::ChatGptSubscription => "chatgpt-responses-v1",
        _ => "liter-chat-v1",
    }
}
