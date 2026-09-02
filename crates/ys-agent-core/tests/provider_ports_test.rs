use ys_agent_core::{
    ActivateProfileRequest, ActiveProviderSnapshot, ActiveProviderView, ActiveRevisionPrecondition,
    CompatibilityEvidenceView, CredentialGeneration, CredentialLease, CredentialMutation,
    CredentialMutationIntent, CredentialMutationRecord, CredentialMutationRequest,
    CredentialPointerCommit, CredentialProtectionStatus, CredentialVault, CredentialViewStatus,
    DeleteProfileRequest, DeviceAuthorizationView, DiscoverModelsRequest, DiscoveredModel,
    ModelDiscovery, OAuthConnectionService, OAuthConnectionView, OperationId, ProfileDetail,
    ProfileId, ProfileName, ProfileRevision, ProfileRevisionRepository, ProfileSummary,
    ProtectedCredentialWrite, ProviderCatalogView, ProviderClientBinding, ProviderClientFactory,
    ProviderCredentialReference, ProviderDoctorView, ProviderErrorCode, ProviderField, ProviderId,
    ProviderManagementApi, ProviderManagementError, ProviderProfileRepository, ProviderRemediation,
    ProviderResult, ResolvedRunProvider, RunId, RunModelProviderResolver,
    RunProviderBindingRepository, SaveProfileRevision, ValidateProfileRequest, ValidationCommit,
};

struct FakeProviderManagementApi;
struct FakeProviderProfileRepository;
struct FakeRunProviderBindingRepository;
struct FakeCredentialVault;
struct FakeProviderClientFactory;
struct FakeModelDiscovery;
struct FakeRunResolver;
struct FakeOAuthConnectionService;

fn unavailable<T>() -> ProviderResult<T> {
    Err(ProviderManagementError::new(
        ProviderErrorCode::Internal,
        None,
        ProviderRemediation::ContactSupport,
    ))
}

#[async_trait::async_trait]
impl ProviderManagementApi for FakeProviderManagementApi {
    async fn catalog(&self) -> ProviderResult<Vec<ProviderCatalogView>> {
        Ok(Vec::new())
    }

    async fn list_profiles(&self) -> ProviderResult<Vec<ProfileSummary>> {
        Ok(Vec::new())
    }

    async fn load_profile(&self, _profile_id: ProfileId) -> ProviderResult<ProfileDetail> {
        unavailable()
    }

    async fn save_profile(
        &self,
        _request: ys_agent_core::SaveProfileRequest,
    ) -> ProviderResult<ProfileDetail> {
        unavailable()
    }

    async fn copy_profile(
        &self,
        _source: ProfileId,
        _name: ProfileName,
    ) -> ProviderResult<ProfileDetail> {
        unavailable()
    }

    async fn mutate_credential(
        &self,
        _request: CredentialMutationRequest,
    ) -> ProviderResult<ProfileDetail> {
        unavailable()
    }

    async fn delete_profile(&self, _request: DeleteProfileRequest) -> ProviderResult<()> {
        unavailable()
    }

    async fn discover_models(
        &self,
        _request: DiscoverModelsRequest,
    ) -> ProviderResult<Vec<DiscoveredModel>> {
        unavailable()
    }

    async fn validate_profile(
        &self,
        _request: ValidateProfileRequest,
    ) -> ProviderResult<CompatibilityEvidenceView> {
        unavailable()
    }

    async fn activate(
        &self,
        _request: ActivateProfileRequest,
    ) -> ProviderResult<ActiveProviderView> {
        unavailable()
    }

    async fn credential_status(
        &self,
        _profile_id: ProfileId,
    ) -> ProviderResult<CredentialViewStatus> {
        unavailable()
    }

    async fn oauth_connection(
        &self,
        _profile_id: ProfileId,
    ) -> ProviderResult<OAuthConnectionView> {
        unavailable()
    }

    async fn doctor(&self) -> ProviderResult<ProviderDoctorView> {
        unavailable()
    }

    async fn cancel_operation(&self, _operation_id: OperationId) -> ProviderResult<()> {
        unavailable()
    }

    async fn start_oauth(
        &self,
        _profile_id: ProfileId,
        _operation_id: OperationId,
    ) -> ProviderResult<DeviceAuthorizationView> {
        unavailable()
    }

    async fn complete_oauth(
        &self,
        _operation_id: OperationId,
    ) -> ProviderResult<OAuthConnectionView> {
        unavailable()
    }

    async fn refresh_oauth(
        &self,
        _profile_id: ProfileId,
        _operation_id: OperationId,
    ) -> ProviderResult<OAuthConnectionView> {
        unavailable()
    }

    async fn reauthorize_oauth(
        &self,
        _profile_id: ProfileId,
        _operation_id: OperationId,
    ) -> ProviderResult<DeviceAuthorizationView> {
        unavailable()
    }

    async fn logout_oauth(
        &self,
        _profile_id: ProfileId,
        _operation_id: OperationId,
    ) -> ProviderResult<ys_agent_core::RemoteRevocationOutcome> {
        unavailable()
    }
}

#[async_trait::async_trait]
impl ProfileRevisionRepository for FakeProviderProfileRepository {
    async fn list_profiles(&self) -> ProviderResult<Vec<ProfileSummary>> {
        Ok(Vec::new())
    }

    async fn load_current_revision(
        &self,
        _profile_id: ProfileId,
    ) -> ProviderResult<ProfileRevision> {
        unavailable()
    }

    async fn load_revision(
        &self,
        _profile_id: ProfileId,
        _revision: u64,
    ) -> ProviderResult<ProfileRevision> {
        unavailable()
    }

    async fn save_revision(
        &self,
        _request: SaveProfileRevision,
    ) -> ProviderResult<ProfileRevision> {
        unavailable()
    }

    async fn active(&self) -> ProviderResult<Option<ActiveProviderSnapshot>> {
        Ok(None)
    }
}

#[async_trait::async_trait]
impl ProviderProfileRepository for FakeProviderProfileRepository {
    async fn save_validation(&self, _commit: ValidationCommit) -> ProviderResult<ProfileRevision> {
        unavailable()
    }

    async fn activate(
        &self,
        _request: ActivateProfileRequest,
    ) -> ProviderResult<ActiveProviderSnapshot> {
        unavailable()
    }

    async fn begin_credential_mutation(
        &self,
        _intent: CredentialMutationIntent,
    ) -> ProviderResult<CredentialMutationRecord> {
        unavailable()
    }

    async fn record_credential_vault_write(
        &self,
        _mutation_id: OperationId,
    ) -> ProviderResult<CredentialMutationRecord> {
        unavailable()
    }

    async fn commit_credential_pointer(
        &self,
        _commit: CredentialPointerCommit,
    ) -> ProviderResult<CredentialMutationRecord> {
        unavailable()
    }

    async fn complete_credential_mutation(
        &self,
        _mutation_id: OperationId,
    ) -> ProviderResult<CredentialMutationRecord> {
        unavailable()
    }

    async fn rollback_credential_mutation(
        &self,
        _mutation_id: OperationId,
    ) -> ProviderResult<CredentialMutationRecord> {
        unavailable()
    }

    async fn block_credential_mutation(
        &self,
        _mutation_id: OperationId,
        _error_code: ProviderErrorCode,
    ) -> ProviderResult<CredentialMutationRecord> {
        unavailable()
    }

    async fn pending_credential_mutations(&self) -> ProviderResult<Vec<CredentialMutationRecord>> {
        Ok(Vec::new())
    }

    async fn delete_profile(&self, _request: DeleteProfileRequest) -> ProviderResult<()> {
        unavailable()
    }
}

#[async_trait::async_trait]
impl RunProviderBindingRepository for FakeRunProviderBindingRepository {
    async fn load_run_binding(
        &self,
        _run_id: RunId,
    ) -> ProviderResult<ys_agent_core::RunProviderBinding> {
        unavailable()
    }

    async fn has_nonterminal_profile_references(
        &self,
        _profile_id: ProfileId,
    ) -> ProviderResult<bool> {
        Ok(false)
    }

    async fn has_nonterminal_credential_references(
        &self,
        _credential: CredentialGeneration,
    ) -> ProviderResult<bool> {
        Ok(false)
    }
}

#[async_trait::async_trait]
impl CredentialVault for FakeCredentialVault {
    async fn protection_status(&self) -> ProviderResult<CredentialProtectionStatus> {
        Ok(CredentialProtectionStatus::ConfirmedNative)
    }

    async fn credential_status(
        &self,
        _reference: ProviderCredentialReference,
    ) -> ProviderResult<CredentialViewStatus> {
        Ok(CredentialViewStatus::Missing)
    }

    async fn write_generation(&self, _input: ProtectedCredentialWrite) -> ProviderResult<()> {
        unavailable()
    }

    async fn read_generation(
        &self,
        _reference: ProviderCredentialReference,
    ) -> ProviderResult<CredentialLease> {
        unavailable()
    }

    async fn delete_generation(
        &self,
        _reference: ProviderCredentialReference,
    ) -> ProviderResult<()> {
        unavailable()
    }
}

#[async_trait::async_trait]
impl ProviderClientFactory for FakeProviderClientFactory {
    async fn build(
        &self,
        _binding: ProviderClientBinding,
        _credential: CredentialLease,
    ) -> ProviderResult<std::sync::Arc<dyn ys_agent_core::ModelProvider>> {
        unavailable()
    }
}

#[async_trait::async_trait]
impl ModelDiscovery for FakeModelDiscovery {
    async fn discover(
        &self,
        _request: DiscoverModelsRequest,
        _credential: CredentialLease,
    ) -> ProviderResult<Vec<DiscoveredModel>> {
        unavailable()
    }
}

#[async_trait::async_trait]
impl RunModelProviderResolver for FakeRunResolver {
    async fn resolve(&self, _run_id: RunId) -> ProviderResult<ResolvedRunProvider> {
        unavailable()
    }
}

#[async_trait::async_trait]
impl OAuthConnectionService for FakeOAuthConnectionService {
    async fn view(&self, _profile_id: ProfileId) -> ProviderResult<OAuthConnectionView> {
        unavailable()
    }

    async fn start(
        &self,
        _profile_id: ProfileId,
        _operation_id: OperationId,
    ) -> ProviderResult<DeviceAuthorizationView> {
        unavailable()
    }

    async fn complete(&self, _operation_id: OperationId) -> ProviderResult<OAuthConnectionView> {
        unavailable()
    }

    async fn refresh(
        &self,
        _profile_id: ProfileId,
        _operation_id: OperationId,
    ) -> ProviderResult<OAuthConnectionView> {
        unavailable()
    }

    async fn reauthorize(
        &self,
        _profile_id: ProfileId,
        _operation_id: OperationId,
    ) -> ProviderResult<DeviceAuthorizationView> {
        unavailable()
    }

    async fn logout(
        &self,
        _profile_id: ProfileId,
        _operation_id: OperationId,
    ) -> ProviderResult<ys_agent_core::RemoteRevocationOutcome> {
        unavailable()
    }
}

#[test]
fn provider_management_errors_are_stable_and_masked_views_carry_only_status() {
    let error = ProviderManagementError::new(
        ProviderErrorCode::CredentialProtectionUnavailable,
        Some(ProviderField::Credential),
        ProviderRemediation::ConfigureCredentialStore,
    );

    assert_eq!(error.code(), "provider.credential.protection_unavailable");
    assert_eq!(
        serde_json::to_value(&error).expect("error serializes")["code"],
        "provider.credential.protection_unavailable"
    );
    assert_eq!(error.category().as_str(), "credential");
    assert_eq!(error.retryability().as_str(), "never");
    assert_eq!(error.field(), Some(&ProviderField::Credential));
    assert_eq!(
        error.remediation(),
        ProviderRemediation::ConfigureCredentialStore
    );

    let status = CredentialViewStatus::Saved;
    let rendered = serde_json::to_string(&status).expect("masked status serializes");
    assert_eq!(rendered, "\"saved\"");
    assert!(!rendered.contains("secret"));

    let first = OperationId::new();
    assert_ne!(first, OperationId::new());
}

#[test]
fn provider_error_contract_covers_retryable_transport_and_protocol_failures() {
    let rate_limit = ProviderManagementError::new(
        ProviderErrorCode::RateLimited,
        None,
        ProviderRemediation::Retry,
    );
    assert_eq!(rate_limit.code(), "provider.rate_limited");
    assert_eq!(rate_limit.category().as_str(), "rate_limit");
    assert_eq!(rate_limit.retryability().as_str(), "bounded");

    let protocol = ProviderManagementError::new(
        ProviderErrorCode::ProtocolInvalidResponse,
        Some(ProviderField::Validation),
        ProviderRemediation::ReturnToEdit,
    );
    assert_eq!(protocol.code(), "provider.protocol.invalid_response");
    assert_eq!(protocol.category().as_str(), "protocol");
    assert_eq!(protocol.retryability().as_str(), "never");
}

#[test]
fn credential_deletion_is_an_explicit_secret_free_service_command() {
    let deletion = CredentialMutation::Delete;
    assert!(matches!(deletion, CredentialMutation::Delete));
}

#[test]
fn repository_and_masked_views_expose_complete_cas_state_without_secrets() {
    fn assert_persistable_revision(revision: &ProfileRevision) {
        let _: ProviderId = revision.provider();
        let _: &ys_agent_core::ProviderModelId = revision.model();
        let _: &ys_agent_core::ProviderParameters = revision.parameters();
        let _: Option<CredentialGeneration> = revision.credential_generation();
        let _: Option<&ys_agent_core::CompatibilityEvidence> = revision.validation();
    }
    let _ = assert_persistable_revision;

    fn assert_edit_snapshot(detail: &ProfileDetail) {
        let _: u64 = detail.revision;
        let _: Option<CredentialGeneration> = detail.credential_generation;
    }
    let _ = assert_edit_snapshot;

    let profile_id = ProfileId::new();
    let credential_free_revision = ProfileRevision::draft(
        profile_id,
        4,
        ProviderId::DeepSeek,
        ys_agent_core::ProviderModelId::new(ProviderId::DeepSeek, "deepseek/model")
            .expect("valid model"),
        ys_agent_core::ProviderParameters::default(),
        None,
    )
    .expect("valid deletion revision");
    let pointer =
        CredentialPointerCommit::new(OperationId::new(), profile_id, 3, credential_free_revision)
            .expect("valid credential deletion pointer");
    assert!(pointer.new_generation().is_none());

    let delete = DeleteProfileRequest {
        operation_id: OperationId::new(),
        profile_id,
        expected_revision: 3,
        expected_active: Some(ActiveRevisionPrecondition {
            profile_id,
            revision: 2,
            activation_revision: 9,
        }),
        enter_no_active_provider: true,
    };
    assert_eq!(delete.expected_revision, 3);
    assert_eq!(delete.expected_active.expect("active CAS").revision, 2);
    assert_eq!(
        delete
            .expected_active
            .expect("active CAS")
            .activation_revision,
        9
    );
}

#[test]
fn model_discovery_accepts_a_draft_without_a_run_or_selected_model() {
    fn assert_draft_request(request: &DiscoverModelsRequest) {
        let _: ProviderId = request.provider;
        let _: u64 = request.profile_revision;
        let _: CredentialGeneration = request.credential_generation;
    }
    let _ = assert_draft_request;

    fn assert_candidate_client_binding(
        revision: &ProfileRevision,
    ) -> ys_agent_core::CoreResult<ProviderClientBinding> {
        ProviderClientBinding::from_revision(revision)
    }
    let _ = assert_candidate_client_binding;
}

#[test]
fn tui_and_doctor_can_compile_against_a_vendor_neutral_fake_service_port() {
    let fake = FakeProviderManagementApi;
    let api: &dyn ProviderManagementApi = &fake;

    // Creating these futures through a trait object proves the TUI/Doctor boundary does not
    // require a store, Vault, OAuth transport, or a vendor client in its test double.
    drop(api.catalog());
    drop(api.copy_profile(
        ProfileId::new(),
        ProfileName::new("copy").expect("valid name"),
    ));
    drop(api.cancel_operation(OperationId::new()));
    drop(api.oauth_connection(ProfileId::new()));
    drop(api.doctor());
}

#[test]
fn store_adapter_runtime_and_tui_can_compile_against_fake_provider_ports() {
    let repository = FakeProviderProfileRepository;
    let bindings = FakeRunProviderBindingRepository;
    let vault = FakeCredentialVault;
    let factory = FakeProviderClientFactory;
    let discovery = FakeModelDiscovery;
    let resolver = FakeRunResolver;
    let oauth = FakeOAuthConnectionService;

    let _: &dyn ProviderProfileRepository = &repository;
    let _: &dyn RunProviderBindingRepository = &bindings;
    let _: &dyn CredentialVault = &vault;
    let _: &dyn ProviderClientFactory = &factory;
    let _: &dyn ModelDiscovery = &discovery;
    let _: &dyn RunModelProviderResolver = &resolver;
    let _: &dyn OAuthConnectionService = &oauth;
}
