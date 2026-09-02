//! Provider Profile lifecycle orchestration.
//!
//! This module owns non-sensitive Profile browsing, Draft revision persistence, and copying. It
//! deliberately has no Vault, OAuth, model client, probe, Query, or deletion authority; those
//! flows arrive in their dedicated tasks.

use std::sync::Arc;

use ys_agent_core::{
    ActiveProviderSnapshot, CredentialViewStatus, ProfileDetail, ProfileId, ProfileName,
    ProfileRevision, ProfileRevisionRepository, ProfileSummary, ProviderErrorCode, ProviderField,
    ProviderManagementError, ProviderRemediation, ProviderResult, RevisionPrecondition,
    SaveProfileRequest, SaveProfileRevision,
};

use super::validation::{LocalProfileValidationRequest, LocalProfileValidator};

/// The Profile-only portion of the Provider-management application service.
///
/// It depends on the narrow revision port so creating, listing, editing, and copying Profiles do
/// not acquire unintended credential mutation or deletion authority.
pub struct ProviderManagementService {
    profiles: Arc<dyn ProfileRevisionRepository>,
    local_validator: LocalProfileValidator,
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
