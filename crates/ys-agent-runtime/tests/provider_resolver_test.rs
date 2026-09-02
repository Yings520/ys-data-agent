use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use ys_agent_core::{
    ActiveProviderSnapshot, CompatibilityEvidence, CoreError, CoreResult, CredentialGeneration,
    CredentialLease, CredentialProtectionStatus, CredentialVault, CredentialViewStatus,
    ModelCapabilities, ModelProvider, ModelRequest, ModelResponse, ProfileId, ProfileRevision,
    ProtectedCredentialWrite, ProviderClientBinding, ProviderClientFactory,
    ProviderCredentialReference, ProviderErrorCode, ProviderField, ProviderId,
    ProviderManagementError, ProviderRemediation, ProviderResult, RunId, RunModelProviderResolver,
    RunProviderBinding, RunProviderBindingRepository, SecretValue, ValidationVersions,
};
use ys_agent_runtime::provider::resolver::RunBoundProviderResolver;

struct Bindings {
    values: HashMap<RunId, RunProviderBinding>,
    statuses: HashMap<CredentialGeneration, CredentialViewStatus>,
}

#[async_trait::async_trait]
impl RunProviderBindingRepository for Bindings {
    async fn load_run_binding(&self, run_id: RunId) -> ProviderResult<RunProviderBinding> {
        self.values
            .get(&run_id)
            .cloned()
            .ok_or_else(missing_binding)
    }

    async fn credential_status(
        &self,
        credential: CredentialGeneration,
    ) -> ProviderResult<CredentialViewStatus> {
        Ok(self
            .statuses
            .get(&credential)
            .copied()
            .unwrap_or(CredentialViewStatus::Missing))
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

#[derive(Default)]
struct Vault {
    statuses: Mutex<HashMap<CredentialGeneration, CredentialViewStatus>>,
    reads: AtomicUsize,
}

impl Vault {
    fn save(&self, generation: CredentialGeneration) {
        self.statuses
            .lock()
            .expect("Vault test state")
            .insert(generation, CredentialViewStatus::Saved);
    }
}

#[async_trait::async_trait]
impl CredentialVault for Vault {
    async fn protection_status(&self) -> ProviderResult<CredentialProtectionStatus> {
        Ok(CredentialProtectionStatus::ConfirmedNative)
    }

    async fn credential_status(
        &self,
        reference: ProviderCredentialReference,
    ) -> ProviderResult<CredentialViewStatus> {
        Ok(self
            .statuses
            .lock()
            .expect("Vault test state")
            .get(&reference.generation)
            .copied()
            .unwrap_or(CredentialViewStatus::Missing))
    }

    async fn write_generation(&self, _input: ProtectedCredentialWrite) -> ProviderResult<()> {
        unreachable!("resolver never writes a credential")
    }

    async fn read_generation(
        &self,
        reference: ProviderCredentialReference,
    ) -> ProviderResult<CredentialLease> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if self
            .statuses
            .lock()
            .expect("Vault test state")
            .get(&reference.generation)
            == Some(&CredentialViewStatus::Saved)
        {
            return Ok(CredentialLease::new(SecretValue::from_utf8(
                "resolver-test-secret".to_owned(),
            )));
        }
        Err(missing_credential())
    }

    async fn delete_generation(
        &self,
        _reference: ProviderCredentialReference,
    ) -> ProviderResult<()> {
        unreachable!("resolver never deletes a credential")
    }
}

#[derive(Default)]
struct Factory {
    builds: AtomicUsize,
    generations: Mutex<Vec<CredentialGeneration>>,
}

struct TestProvider;

#[async_trait::async_trait]
impl ModelProvider for TestProvider {
    fn capabilities(&self) -> ModelCapabilities {
        ModelCapabilities::default()
    }

    async fn complete(&self, _request: ModelRequest) -> CoreResult<ModelResponse> {
        Err(CoreError::validation(
            "unexpected_model_call",
            "resolver test provider is never called",
        ))
    }
}

#[async_trait::async_trait]
impl ProviderClientFactory for Factory {
    async fn build(
        &self,
        binding: ProviderClientBinding,
        credential: CredentialLease,
    ) -> ProviderResult<Arc<dyn ModelProvider>> {
        credential.with_secret(|_| ());
        self.builds.fetch_add(1, Ordering::SeqCst);
        self.generations
            .lock()
            .expect("Factory test state")
            .push(binding.credential_generation);
        tokio::time::sleep(Duration::from_millis(20)).await;
        Ok(Arc::new(TestProvider))
    }
}

fn binding(run_id: RunId, profile_id: ProfileId, generation: u64) -> RunProviderBinding {
    let credential = CredentialGeneration::new(
        profile_id,
        generation,
        ProviderId::DeepSeek.required_credential_kind(),
    )
    .expect("matching credential generation");
    let mut revision = ProfileRevision::draft(
        profile_id,
        1,
        ProviderId::DeepSeek,
        ys_agent_core::ProviderModelId::new(ProviderId::DeepSeek, "deepseek/resolver-model")
            .expect("governed model"),
        ys_agent_core::ProviderParameters::default(),
        Some(credential),
    )
    .expect("valid Draft");
    let versions = ValidationVersions::new("catalog", "probe", "liter", "codec");
    let evidence = CompatibilityEvidence::passing(revision.validation_inputs(versions.clone()));
    revision
        .accept_validation(evidence, versions)
        .expect("ready revision");
    RunProviderBinding::from_active(
        run_id,
        ActiveProviderSnapshot::from_ready(&revision, 1).expect("active snapshot"),
    )
    .expect("immutable Run binding")
}

fn missing_binding() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::StorageConflict,
        Some(ProviderField::Provider),
        ProviderRemediation::WaitForCurrentOperation,
    )
}

fn missing_credential() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::CredentialMissing,
        Some(ProviderField::Credential),
        ProviderRemediation::ConfigureCredentialStore,
    )
}

#[tokio::test]
async fn resolver_single_flights_one_run_and_releases_its_run_scoped_client() {
    let run_id = RunId::new();
    let profile_id = ProfileId::new();
    let binding = binding(run_id, profile_id, 1);
    let vault = Arc::new(Vault::default());
    vault.save(binding.credential_generation());
    let factory = Arc::new(Factory::default());
    let resolver = Arc::new(RunBoundProviderResolver::new(
        Arc::new(Bindings {
            values: HashMap::from([(run_id, binding.clone())]),
            statuses: HashMap::from([(
                binding.credential_generation(),
                CredentialViewStatus::Saved,
            )]),
        }),
        vault.clone(),
        factory.clone(),
    ));

    let (first, second) = tokio::join!(resolver.resolve(run_id), resolver.resolve(run_id));
    assert_eq!(first.expect("first resolution").binding, binding);
    assert_eq!(second.expect("second resolution").binding, binding);
    assert_eq!(factory.builds.load(Ordering::SeqCst), 1);
    assert_eq!(vault.reads.load(Ordering::SeqCst), 1);

    resolver.release_run(run_id).await;
    resolver
        .resolve(run_id)
        .await
        .expect("resolve after release");
    assert_eq!(factory.builds.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn resolver_uses_only_each_binding_generation_and_fails_before_factory_when_unavailable() {
    let profile_id = ProfileId::new();
    let first_run = RunId::new();
    let second_run = RunId::new();
    let first = binding(first_run, profile_id, 1);
    let second = binding(second_run, profile_id, 2);
    let vault = Arc::new(Vault::default());
    vault.save(first.credential_generation());
    vault.save(second.credential_generation());
    let factory = Arc::new(Factory::default());
    let resolver = RunBoundProviderResolver::new(
        Arc::new(Bindings {
            values: HashMap::from([(first_run, first.clone()), (second_run, second.clone())]),
            statuses: HashMap::from([
                (first.credential_generation(), CredentialViewStatus::Saved),
                (
                    second.credential_generation(),
                    CredentialViewStatus::Revoked,
                ),
            ]),
        }),
        vault.clone(),
        factory.clone(),
    );

    let resolved = resolver
        .resolve(first_run)
        .await
        .expect("first binding resolves");
    assert_eq!(
        resolved.binding.credential_generation(),
        first.credential_generation()
    );
    let unavailable = match resolver.resolve(second_run).await {
        Ok(_) => panic!("revoked exact generation cannot fall back to another generation"),
        Err(error) => error,
    };
    assert_eq!(
        unavailable.code(),
        ProviderErrorCode::AuthenticationInvalid.as_str()
    );
    assert_eq!(factory.builds.load(Ordering::SeqCst), 1);
    assert_eq!(
        factory
            .generations
            .lock()
            .expect("Factory test state")
            .as_slice(),
        &[first.credential_generation()]
    );
    assert_eq!(vault.reads.load(Ordering::SeqCst), 1);
}
