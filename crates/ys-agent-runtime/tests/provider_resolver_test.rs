use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use tempfile::TempDir;
use ys_agent_adapters::credential::keyring::InMemoryCredentialVault;
use ys_agent_core::{
    ActivateProfileRequest, ActivationPrecondition, ActiveProviderSnapshot, CompatibilityEvidence,
    ContextManifest, CoreError, CoreResult, CredentialGeneration, CredentialKind, CredentialLease,
    CredentialMutation, CredentialMutationIntent, CredentialMutationRequest,
    CredentialProtectionStatus, CredentialVault, CredentialViewStatus, ModelCapabilities,
    ModelProvider, ModelRequest, ModelResponse, OperationId, ProfileId, ProfileRevision,
    ProtectedCredentialWrite, ProviderClientBinding, ProviderClientFactory,
    ProviderCredentialReference, ProviderErrorCode, ProviderField, ProviderId,
    ProviderManagementError, ProviderRemediation, ProviderResult, RunId, RunModelProviderResolver,
    RunProviderBinding, RunProviderBindingRepository, RunProviderBindingSource, SecretValue,
    ValidationCommit, ValidationCommitPrecondition, ValidationVersions,
};
use ys_agent_runtime::{
    ActiveRunProviderBindingSource,
    provider::{
        resolver::RunBoundProviderResolver,
        service::{CredentialService, ProviderManagementService},
    },
};
use ys_agent_store::SqliteRuntimeStore;

#[path = "support/provider_fixture.rs"]
mod provider_fixture;

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

#[tokio::test]
async fn active_rotation_never_redirects_concurrent_run_bindings() {
    exercise_rotated_run_bindings().await;
}

async fn exercise_rotated_run_bindings() {
    let directory = TempDir::new().expect("temporary database directory");
    let store = SqliteRuntimeStore::open(directory.path().join("runtime.db"))
        .await
        .expect("open durable Provider store");
    let repository = store.provider_repository();
    let original = provider_fixture::persisted_test_active_provider(&store).await;
    let profile_id = original.profile_id();
    let first_generation = original.credential_generation();
    let vault = Arc::new(InMemoryCredentialVault::new());
    vault
        .write_generation(ProtectedCredentialWrite {
            reference: ProviderCredentialReference {
                profile_id,
                generation: first_generation,
            },
            secret: SecretValue::from_utf8("run-a-fixture-credential".to_owned()),
        })
        .await
        .expect("seed only the protected first generation");

    let source = ActiveRunProviderBindingSource::new(
        Arc::new(repository.clone()),
        Arc::new(store.run_binding_repository()),
        vault.clone(),
    );
    let run_a = RunId::new();
    let binding_a = source
        .bind_new_run(run_a)
        .await
        .expect("Run A binds the initial active snapshot");
    assert_eq!(binding_a.profile_revision(), original.profile_revision());
    assert_eq!(binding_a.credential_generation(), first_generation);

    let second_generation = CredentialGeneration::new(profile_id, 2, CredentialKind::ApiKey)
        .expect("second API-key generation");
    let credentials = CredentialService::new(
        Arc::new(repository.clone()),
        Arc::new(store.run_binding_repository()),
        vault.clone(),
    );
    let rotated = credentials
        .mutate(CredentialMutationRequest {
            intent: CredentialMutationIntent::replace(
                OperationId::new(),
                profile_id,
                binding_a.profile_revision(),
                first_generation,
                second_generation,
            )
            .expect("rotation follows Run A's original revision"),
            mutation: CredentialMutation::Replace(ProtectedCredentialWrite {
                reference: ProviderCredentialReference {
                    profile_id,
                    generation: second_generation,
                },
                secret: SecretValue::from_utf8("run-b-fixture-credential".to_owned()),
            }),
        })
        .await
        .expect("rotation appends an unvalidated revision without moving Run A");
    assert_eq!(rotated.revision, binding_a.profile_revision() + 1);
    assert_eq!(rotated.credential_generation, Some(second_generation));

    let candidate = repository
        .load_current_revision(profile_id)
        .await
        .expect("rotated Draft is current");
    let versions =
        ValidationVersions::new("race-catalog", "race-probe", "race-liter", "race-codec");
    let evidence = CompatibilityEvidence::passing(candidate.validation_inputs(versions.clone()));
    let validation_id = evidence.id();
    let validation_digest = evidence.digest();
    let profiles = ProviderManagementService::new(Arc::new(repository.clone()));
    profiles
        .commit_validation(ValidationCommit {
            precondition: ValidationCommitPrecondition {
                operation_id: OperationId::new(),
                profile_id,
                revision: candidate.revision(),
                credential_generation: second_generation,
                validation_digest: validation_digest.clone(),
            },
            evidence,
            versions,
        })
        .await
        .expect("the rotated generation receives its own validation evidence");
    let active_b = profiles
        .activate(ActivateProfileRequest {
            operation_id: OperationId::new(),
            precondition: ActivationPrecondition {
                profile_id,
                revision: candidate.revision(),
                validation_id,
                validation_digest,
                expected_activation_revision: Some(binding_a.activation_revision()),
            },
        })
        .await
        .expect("activate revision two only for future Runs");

    let run_b = RunId::new();
    let binding_b = source
        .bind_new_run(run_b)
        .await
        .expect("Run B reads the newly active snapshot");
    assert_eq!(binding_b.profile_revision(), active_b.profile_revision);
    assert_eq!(binding_b.credential_generation(), second_generation);
    assert_ne!(binding_a.fingerprint(), binding_b.fingerprint());

    let factory = Arc::new(Factory::default());
    let resolver = Arc::new(RunBoundProviderResolver::new(
        Arc::new(Bindings {
            values: HashMap::from([(run_a, binding_a.clone()), (run_b, binding_b.clone())]),
            statuses: HashMap::from([
                (first_generation, CredentialViewStatus::Saved),
                (second_generation, CredentialViewStatus::Saved),
            ]),
        }),
        vault,
        factory.clone(),
    ));

    let (a_probe, a_retry, b_probe) = tokio::join!(
        resolver.resolve(run_a),
        resolver.resolve(run_a),
        resolver.resolve(run_b),
    );
    let a_probe = a_probe.expect("Run A concurrent probe resolves its exact original binding");
    let a_retry = a_retry.expect("Run A retry single-flights without rebinding");
    let b_probe = b_probe.expect("Run B concurrent probe resolves its rotated binding");
    assert_eq!(a_probe.binding, binding_a);
    assert_eq!(a_retry.binding, binding_a);
    assert_eq!(b_probe.binding, binding_b);
    assert_eq!(factory.builds.load(Ordering::SeqCst), 2);
    assert_eq!(
        factory
            .generations
            .lock()
            .expect("Factory test state")
            .iter()
            .filter(|generation| **generation == second_generation)
            .count(),
        1,
        "Run B constructs only its own rotated credential generation"
    );

    let failed_call = a_probe
        .provider
        .complete(ModelRequest {
            model: "deepseek/fixture".to_owned(),
            messages: Vec::new(),
            tools: Vec::new(),
            context_manifest: ContextManifest::empty(1),
            temperature: None,
        })
        .await
        .expect_err("a Provider failure must remain visible rather than route Run A elsewhere");
    assert_eq!(failed_call.code(), "unexpected_model_call");

    resolver.release_run(run_a).await;
    let after_failure = resolver
        .resolve(run_a)
        .await
        .expect("a released Run A cache reloads only its persisted binding");
    assert_eq!(after_failure.binding, binding_a);
    assert_eq!(
        after_failure.binding.credential_generation(),
        first_generation
    );
    assert_eq!(factory.builds.load(Ordering::SeqCst), 3);
    assert_eq!(
        factory
            .generations
            .lock()
            .expect("Factory test state")
            .iter()
            .filter(|generation| **generation == first_generation)
            .count(),
        2,
        "Run A never reads the newer Run B credential after activation, retry, or failure"
    );
}
