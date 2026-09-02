//! Immutable Run-bound Provider resolution.
//!
//! This resolver deliberately has no Profile or active-provider dependency. A Run can only use
//! the exact binding written with it, so later edits, activation changes, and credential rotation
//! cannot redirect an in-flight model call.

use std::{collections::HashMap, sync::Arc};

use tokio::sync::{Mutex, OnceCell};
use ys_agent_core::{
    CredentialVault, CredentialViewStatus, ModelProvider, ProviderClientBinding,
    ProviderClientFactory, ProviderCredentialReference, ProviderErrorCode, ProviderField,
    ProviderManagementError, ProviderRemediation, ProviderResult, ResolvedRunProvider, RunId,
    RunModelProviderResolver, RunProviderBindingRepository,
};

/// Resolves one immutable Run binding into its one exact model client.
///
/// The cache is keyed by both Run and the binding fingerprint. It therefore deduplicates only
/// concurrent resolution of the same Run and is explicitly released when that Run terminates.
pub struct RunBoundProviderResolver {
    bindings: Arc<dyn RunProviderBindingRepository>,
    vault: Arc<dyn CredentialVault>,
    factory: Arc<dyn ProviderClientFactory>,
    cache: Mutex<HashMap<RunId, CachedProvider>>,
}

/// Explicit composition adapter for deterministic or transitional assemblies. It still loads the
/// exact persisted Run binding, so the Harness never reads the active Profile or selects a model
/// name from bootstrap state.
pub struct FixedRunModelProviderResolver {
    bindings: Arc<dyn RunProviderBindingRepository>,
    provider: Arc<dyn ModelProvider>,
}

impl FixedRunModelProviderResolver {
    pub fn new(
        bindings: Arc<dyn RunProviderBindingRepository>,
        provider: Arc<dyn ModelProvider>,
    ) -> Self {
        Self { bindings, provider }
    }
}

struct CachedProvider {
    binding_digest: String,
    provider: Arc<OnceCell<Arc<dyn ModelProvider>>>,
}

impl RunBoundProviderResolver {
    pub fn new(
        bindings: Arc<dyn RunProviderBindingRepository>,
        vault: Arc<dyn CredentialVault>,
        factory: Arc<dyn ProviderClientFactory>,
    ) -> Self {
        Self {
            bindings,
            vault,
            factory,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Removes only this terminal Run's in-memory client. No secret, Vault record, Profile, or
    /// binding is altered; historical Run explanation remains durable in the binding repository.
    pub async fn release_run(&self, run_id: RunId) {
        self.cache.lock().await.remove(&run_id);
    }

    async fn resolve_binding(&self, run_id: RunId) -> ProviderResult<ResolvedRunProvider> {
        let binding = self.bindings.load_run_binding(run_id).await?;
        if binding.run_id() != run_id {
            return Err(binding_error());
        }
        let binding_digest = binding.fingerprint().digest().to_owned();

        let cell = {
            let mut cache = self.cache.lock().await;
            match cache.get(&run_id) {
                Some(cached) if cached.binding_digest == binding_digest => cached.provider.clone(),
                _ => {
                    let provider = Arc::new(OnceCell::new());
                    cache.insert(
                        run_id,
                        CachedProvider {
                            binding_digest,
                            provider: provider.clone(),
                        },
                    );
                    provider
                }
            }
        };
        let provider = cell
            .get_or_try_init(|| async { self.build_provider(&binding).await })
            .await?
            .clone();
        Ok(ResolvedRunProvider { binding, provider })
    }

    async fn build_provider(
        &self,
        binding: &ys_agent_core::RunProviderBinding,
    ) -> ProviderResult<Arc<dyn ModelProvider>> {
        let generation = binding.credential_generation();
        let reference = ProviderCredentialReference {
            profile_id: binding.profile_id(),
            generation,
        };
        ensure_usable_credential(self.bindings.credential_status(generation).await?)?;
        ensure_usable_credential(self.vault.credential_status(reference.clone()).await?)?;
        let lease = self.vault.read_generation(reference).await?;
        self.factory
            .build(ProviderClientBinding::from_run_binding(binding), lease)
            .await
    }
}

#[async_trait::async_trait]
impl RunModelProviderResolver for RunBoundProviderResolver {
    async fn resolve(&self, run_id: RunId) -> ProviderResult<ResolvedRunProvider> {
        self.resolve_binding(run_id).await
    }
}

#[async_trait::async_trait]
impl RunModelProviderResolver for FixedRunModelProviderResolver {
    async fn resolve(&self, run_id: RunId) -> ProviderResult<ResolvedRunProvider> {
        let binding = self.bindings.load_run_binding(run_id).await?;
        if binding.run_id() != run_id {
            return Err(binding_error());
        }
        Ok(ResolvedRunProvider {
            binding,
            provider: self.provider.clone(),
        })
    }
}

fn ensure_usable_credential(status: CredentialViewStatus) -> ProviderResult<()> {
    match status {
        CredentialViewStatus::Saved => Ok(()),
        CredentialViewStatus::Missing => Err(ProviderManagementError::new(
            ProviderErrorCode::CredentialMissing,
            Some(ProviderField::Credential),
            ProviderRemediation::ConfigureCredentialStore,
        )),
        CredentialViewStatus::Expired | CredentialViewStatus::Revoked => {
            Err(ProviderManagementError::new(
                ProviderErrorCode::AuthenticationInvalid,
                Some(ProviderField::Credential),
                ProviderRemediation::Reauthorize,
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

fn binding_error() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::StorageConflict,
        Some(ProviderField::Provider),
        ProviderRemediation::WaitForCurrentOperation,
    )
}
