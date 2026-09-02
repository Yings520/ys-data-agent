//! OS-native credential storage and its explicit in-memory contract-test replacement.

use std::{
    collections::BTreeMap,
    fmt,
    sync::{Arc, Mutex, MutexGuard},
};

use async_trait::async_trait;
use serde::{Deserialize, Deserializer, Serialize};
use tokio::sync::OnceCell;
use uuid::Uuid;
use ys_agent_core::{
    CredentialKind, CredentialLease, CredentialProtectionStatus, CredentialVault,
    CredentialViewStatus, ProfileId, ProtectedCredentialWrite, ProviderCredentialReference,
    ProviderErrorCode, ProviderField, ProviderManagementError, ProviderRemediation, ProviderResult,
    SecretValue,
};
use zeroize::{Zeroize, Zeroizing};

/// Stable service identifier used by every production credential entry.
pub const KEYRING_SERVICE: &str = "io.ysda.provider";

const ENVELOPE_VERSION: u32 = 1;
const PROBE_ACCOUNT_PREFIX: &str = "__native_protection_probe__";
static KEYRING_MUTATION_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone)]
pub struct KeyringCredentialVault {
    engine: VaultEngine,
}

impl KeyringCredentialVault {
    /// Construct the production vault. The native protection probe is performed lazily by
    /// [`CredentialVault::protection_status`], so startup wiring can explicitly await it.
    pub fn new() -> Self {
        Self {
            engine: VaultEngine::new(Arc::new(KeyringBackend)),
        }
    }
}

impl Default for KeyringCredentialVault {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for KeyringCredentialVault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("KeyringCredentialVault")
            .field("service", &KEYRING_SERVICE)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl CredentialVault for KeyringCredentialVault {
    async fn protection_status(&self) -> ProviderResult<CredentialProtectionStatus> {
        self.engine.protection_status().await
    }

    async fn credential_status(
        &self,
        reference: ProviderCredentialReference,
    ) -> ProviderResult<CredentialViewStatus> {
        self.engine.credential_status(reference).await
    }

    async fn write_generation(&self, input: ProtectedCredentialWrite) -> ProviderResult<()> {
        self.engine.write_generation(input).await
    }

    async fn read_generation(
        &self,
        reference: ProviderCredentialReference,
    ) -> ProviderResult<CredentialLease> {
        self.engine.read_generation(reference).await
    }

    async fn delete_generation(
        &self,
        reference: ProviderCredentialReference,
    ) -> ProviderResult<()> {
        self.engine.delete_generation(reference).await
    }
}

/// Operations supported by the explicit in-memory contract-test vault's one-shot fault injector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InMemoryVaultOperation {
    Write,
    Read,
    Delete,
}

/// Explicit test replacement for the OS vault. It is never selected by
/// [`KeyringCredentialVault`] and therefore cannot become a production fallback.
#[derive(Clone)]
pub struct InMemoryCredentialVault {
    engine: VaultEngine,
    backend: Arc<InMemoryBackend>,
}

impl InMemoryCredentialVault {
    pub fn new() -> Self {
        Self::with_protection(CredentialProtectionStatus::ConfirmedNative)
    }

    pub fn with_protection(protection: CredentialProtectionStatus) -> Self {
        let backend = Arc::new(InMemoryBackend::new(protection));
        Self {
            engine: VaultEngine::new(backend.clone()),
            backend,
        }
    }

    /// Simulate a process restart while retaining the external credential store.
    pub fn restart(&self) -> Self {
        Self {
            engine: VaultEngine::new(self.backend.clone()),
            backend: self.backend.clone(),
        }
    }

    pub fn fail_next(&self, operation: InMemoryVaultOperation) {
        self.backend.lock().fail_next = Some(operation);
    }

    /// Return non-sensitive account locators for isolation assertions.
    pub fn stored_accounts(&self) -> Vec<String> {
        self.backend.lock().entries.keys().cloned().collect()
    }
}

impl Default for InMemoryCredentialVault {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for InMemoryCredentialVault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InMemoryCredentialVault")
            .field("entry_count", &self.backend.lock().entries.len())
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl CredentialVault for InMemoryCredentialVault {
    async fn protection_status(&self) -> ProviderResult<CredentialProtectionStatus> {
        self.engine.protection_status().await
    }

    async fn credential_status(
        &self,
        reference: ProviderCredentialReference,
    ) -> ProviderResult<CredentialViewStatus> {
        self.engine.credential_status(reference).await
    }

    async fn write_generation(&self, input: ProtectedCredentialWrite) -> ProviderResult<()> {
        self.engine.write_generation(input).await
    }

    async fn read_generation(
        &self,
        reference: ProviderCredentialReference,
    ) -> ProviderResult<CredentialLease> {
        self.engine.read_generation(reference).await
    }

    async fn delete_generation(
        &self,
        reference: ProviderCredentialReference,
    ) -> ProviderResult<()> {
        self.engine.delete_generation(reference).await
    }
}

#[derive(Clone)]
struct VaultEngine {
    backend: Arc<dyn BlockingCredentialBackend>,
    protection: Arc<OnceCell<CredentialProtectionStatus>>,
}

impl VaultEngine {
    fn new(backend: Arc<dyn BlockingCredentialBackend>) -> Self {
        Self {
            backend,
            protection: Arc::new(OnceCell::new()),
        }
    }

    async fn protection_status(&self) -> ProviderResult<CredentialProtectionStatus> {
        let backend = self.backend.clone();
        let status = self
            .protection
            .get_or_init(|| async move { native_protection_probe(backend).await })
            .await;
        Ok(*status)
    }

    async fn require_confirmed_protection(&self) -> ProviderResult<()> {
        if self.protection_status().await? == CredentialProtectionStatus::ConfirmedNative {
            Ok(())
        } else {
            Err(protection_unavailable())
        }
    }

    async fn credential_status(
        &self,
        reference: ProviderCredentialReference,
    ) -> ProviderResult<CredentialViewStatus> {
        if self.protection_status().await? != CredentialProtectionStatus::ConfirmedNative {
            return Ok(CredentialViewStatus::ProtectionUnavailable);
        }
        let locator = ValidatedLocator::new(reference)?;
        let backend = self.backend.clone();
        let locator_for_read = locator.clone();
        match run_blocking(move || backend.read(&locator_for_read.account)).await? {
            Ok(raw) => {
                drop(decode_envelope(raw, &locator)?);
                Ok(CredentialViewStatus::Saved)
            }
            Err(BackendError::Missing) => Ok(CredentialViewStatus::Missing),
            Err(error) => Err(map_backend_error(error)),
        }
    }

    async fn write_generation(&self, input: ProtectedCredentialWrite) -> ProviderResult<()> {
        self.require_confirmed_protection().await?;
        let locator = ValidatedLocator::new(input.reference)?;
        let encoded = input
            .secret
            .with_exposed(|secret| encode_envelope(&locator, secret))?;
        let backend = self.backend.clone();
        run_blocking(move || backend.write_new(&locator.account, &encoded))
            .await?
            .map_err(map_backend_error)
    }

    async fn read_generation(
        &self,
        reference: ProviderCredentialReference,
    ) -> ProviderResult<CredentialLease> {
        self.require_confirmed_protection().await?;
        let locator = ValidatedLocator::new(reference)?;
        let backend = self.backend.clone();
        let locator_for_read = locator.clone();
        let raw = run_blocking(move || backend.read(&locator_for_read.account))
            .await?
            .map_err(map_backend_error)?;
        let secret = decode_envelope(raw, &locator)?;
        Ok(CredentialLease::new(secret))
    }

    async fn delete_generation(
        &self,
        reference: ProviderCredentialReference,
    ) -> ProviderResult<()> {
        self.require_confirmed_protection().await?;
        let locator = ValidatedLocator::new(reference)?;
        let backend = self.backend.clone();
        match run_blocking(move || backend.delete(&locator.account)).await? {
            Ok(()) | Err(BackendError::Missing) => Ok(()),
            Err(error) => Err(map_backend_error(error)),
        }
    }
}

async fn native_protection_probe(
    backend: Arc<dyn BlockingCredentialBackend>,
) -> CredentialProtectionStatus {
    let account = format!("{PROBE_ACCOUNT_PREFIX}:{}", Uuid::new_v4());
    let proof = Zeroizing::new(Uuid::new_v4().as_bytes().to_vec());
    tokio::task::spawn_blocking(move || {
        let declared = backend.declared_protection();
        if declared != CredentialProtectionStatus::ConfirmedNative {
            return declared;
        }
        if let Err(error) = backend.write_new(&account, &proof) {
            return protection_status_for_backend_error(error);
        }

        let read_result = backend.read(&account);
        let delete_result = backend.delete(&account);
        match (read_result, delete_result) {
            (Ok(read), Ok(())) if read.as_slice() == proof.as_slice() => {
                CredentialProtectionStatus::ConfirmedNative
            }
            (Err(BackendError::Unavailable), _) | (_, Err(BackendError::Unavailable)) => {
                CredentialProtectionStatus::Unavailable
            }
            _ => CredentialProtectionStatus::Unconfirmed,
        }
    })
    .await
    .unwrap_or(CredentialProtectionStatus::Unconfirmed)
}

async fn run_blocking<T>(operation: impl FnOnce() -> T + Send + 'static) -> ProviderResult<T>
where
    T: Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|_| internal_error())
}

#[derive(Clone)]
struct ValidatedLocator {
    account: String,
    profile_id: ProfileId,
    generation: u64,
    tag: EnvelopeTag,
}

impl ValidatedLocator {
    fn new(reference: ProviderCredentialReference) -> ProviderResult<Self> {
        if reference.profile_id != reference.generation.profile_id() {
            return Err(storage_conflict());
        }
        Ok(Self {
            account: format!("{}:{}", reference.profile_id, reference.generation.number()),
            profile_id: reference.profile_id,
            generation: reference.generation.number(),
            tag: EnvelopeTag::from(reference.generation.kind()),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum EnvelopeTag {
    ApiKey,
    OAuthConnection,
}

impl From<CredentialKind> for EnvelopeTag {
    fn from(kind: CredentialKind) -> Self {
        match kind {
            CredentialKind::ApiKey => Self::ApiKey,
            CredentialKind::OAuthConnection => Self::OAuthConnection,
        }
    }
}

#[derive(Serialize)]
struct EnvelopeWrite<'a> {
    version: u32,
    tag: EnvelopeTag,
    profile_id: ProfileId,
    generation: u64,
    payload: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EnvelopeRead {
    version: u32,
    tag: EnvelopeTag,
    profile_id: ProfileId,
    generation: u64,
    payload: SensitiveString,
}

struct SensitiveString(String);

impl SensitiveString {
    fn take(&mut self) -> String {
        std::mem::take(&mut self.0)
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[cfg(test)]
    fn clear(&mut self) {
        self.0.zeroize();
    }
}

impl Drop for SensitiveString {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl<'de> Deserialize<'de> for SensitiveString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(Self)
    }
}

fn encode_envelope(locator: &ValidatedLocator, secret: &str) -> ProviderResult<Zeroizing<Vec<u8>>> {
    if secret.is_empty() {
        return Err(storage_conflict());
    }
    serde_json::to_vec(&EnvelopeWrite {
        version: ENVELOPE_VERSION,
        tag: locator.tag,
        profile_id: locator.profile_id,
        generation: locator.generation,
        payload: secret,
    })
    .map(Zeroizing::new)
    .map_err(|_| internal_error())
}

fn decode_envelope(
    raw: Zeroizing<Vec<u8>>,
    locator: &ValidatedLocator,
) -> ProviderResult<SecretValue> {
    let mut envelope: EnvelopeRead =
        serde_json::from_slice(&raw).map_err(|_| storage_conflict())?;
    if envelope.version != ENVELOPE_VERSION
        || envelope.tag != locator.tag
        || envelope.profile_id != locator.profile_id
        || envelope.generation != locator.generation
        || envelope.payload.is_empty()
    {
        return Err(storage_conflict());
    }
    Ok(SecretValue::from_utf8(envelope.payload.take()))
}

trait BlockingCredentialBackend: Send + Sync {
    fn declared_protection(&self) -> CredentialProtectionStatus;
    fn write_new(&self, account: &str, value: &[u8]) -> Result<(), BackendError>;
    fn read(&self, account: &str) -> Result<Zeroizing<Vec<u8>>, BackendError>;
    fn delete(&self, account: &str) -> Result<(), BackendError>;
}

struct KeyringBackend;

impl BlockingCredentialBackend for KeyringBackend {
    fn declared_protection(&self) -> CredentialProtectionStatus {
        #[cfg(all(any(unix, windows), not(any(target_os = "ios", target_os = "android"))))]
        {
            if keyring::Entry::store_status().is_ok() {
                CredentialProtectionStatus::ConfirmedNative
            } else {
                CredentialProtectionStatus::Unavailable
            }
        }
        #[cfg(not(all(any(unix, windows), not(any(target_os = "ios", target_os = "android")))))]
        {
            CredentialProtectionStatus::Unavailable
        }
    }

    fn write_new(&self, account: &str, value: &[u8]) -> Result<(), BackendError> {
        let _guard = KEYRING_MUTATION_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = keyring::Entry::new(KEYRING_SERVICE, account).map_err(map_keyring_error)?;
        match entry.get_secret() {
            Ok(mut existing) => {
                existing.zeroize();
                Err(BackendError::Conflict)
            }
            Err(keyring::Error::NoEntry) => entry.set_secret(value).map_err(map_keyring_error),
            Err(error) => Err(map_keyring_error(error)),
        }
    }

    fn read(&self, account: &str) -> Result<Zeroizing<Vec<u8>>, BackendError> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, account).map_err(map_keyring_error)?;
        entry
            .get_secret()
            .map(Zeroizing::new)
            .map_err(map_keyring_error)
    }

    fn delete(&self, account: &str) -> Result<(), BackendError> {
        let _guard = KEYRING_MUTATION_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = keyring::Entry::new(KEYRING_SERVICE, account).map_err(map_keyring_error)?;
        entry.delete_credential().map_err(map_keyring_error)
    }
}

#[derive(Clone, Copy)]
enum BackendError {
    Missing,
    Unavailable,
    Conflict,
    Failed,
}

fn map_keyring_error(error: keyring::Error) -> BackendError {
    match error {
        keyring::Error::NoEntry => BackendError::Missing,
        keyring::Error::BadEncoding(mut raw) => {
            raw.zeroize();
            BackendError::Failed
        }
        keyring::Error::BadDataFormat(mut raw, _) => {
            raw.zeroize();
            BackendError::Failed
        }
        keyring::Error::NoDefaultStore
        | keyring::Error::NoStorageAccess(_)
        | keyring::Error::NotSupportedByStore(_) => BackendError::Unavailable,
        _ => BackendError::Failed,
    }
}

struct InMemoryBackend {
    protection: CredentialProtectionStatus,
    state: Mutex<InMemoryState>,
}

impl InMemoryBackend {
    fn new(protection: CredentialProtectionStatus) -> Self {
        Self {
            protection,
            state: Mutex::new(InMemoryState::default()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, InMemoryState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[derive(Default)]
struct InMemoryState {
    entries: BTreeMap<String, Vec<u8>>,
    fail_next: Option<InMemoryVaultOperation>,
}

impl Drop for InMemoryState {
    fn drop(&mut self) {
        for value in self.entries.values_mut() {
            value.zeroize();
        }
    }
}

impl BlockingCredentialBackend for InMemoryBackend {
    fn declared_protection(&self) -> CredentialProtectionStatus {
        self.protection
    }

    fn write_new(&self, account: &str, value: &[u8]) -> Result<(), BackendError> {
        let mut state = self.lock();
        if take_fault(&mut state, InMemoryVaultOperation::Write) {
            return Err(BackendError::Failed);
        }
        if state.entries.contains_key(account) {
            return Err(BackendError::Conflict);
        }
        state.entries.insert(account.to_owned(), value.to_vec());
        Ok(())
    }

    fn read(&self, account: &str) -> Result<Zeroizing<Vec<u8>>, BackendError> {
        let mut state = self.lock();
        if take_fault(&mut state, InMemoryVaultOperation::Read) {
            return Err(BackendError::Failed);
        }
        state
            .entries
            .get(account)
            .cloned()
            .map(Zeroizing::new)
            .ok_or(BackendError::Missing)
    }

    fn delete(&self, account: &str) -> Result<(), BackendError> {
        let mut state = self.lock();
        if take_fault(&mut state, InMemoryVaultOperation::Delete) {
            return Err(BackendError::Failed);
        }
        if let Some(mut value) = state.entries.remove(account) {
            value.zeroize();
        }
        Ok(())
    }
}

fn take_fault(state: &mut InMemoryState, operation: InMemoryVaultOperation) -> bool {
    if state.fail_next == Some(operation) {
        state.fail_next = None;
        true
    } else {
        false
    }
}

fn protection_status_for_backend_error(error: BackendError) -> CredentialProtectionStatus {
    match error {
        BackendError::Unavailable => CredentialProtectionStatus::Unavailable,
        _ => CredentialProtectionStatus::Unconfirmed,
    }
}

fn map_backend_error(error: BackendError) -> ProviderManagementError {
    match error {
        BackendError::Missing => credential_missing(),
        BackendError::Conflict => storage_conflict(),
        BackendError::Unavailable | BackendError::Failed => protection_unavailable(),
    }
}

fn credential_missing() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::CredentialMissing,
        Some(ProviderField::Credential),
        ProviderRemediation::ReturnToEdit,
    )
}

fn protection_unavailable() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::CredentialProtectionUnavailable,
        Some(ProviderField::Credential),
        ProviderRemediation::ConfigureCredentialStore,
    )
}

fn storage_conflict() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::StorageConflict,
        Some(ProviderField::Credential),
        ProviderRemediation::ReturnToEdit,
    )
}

fn internal_error() -> ProviderManagementError {
    ProviderManagementError::new(
        ProviderErrorCode::Internal,
        Some(ProviderField::Credential),
        ProviderRemediation::ContactSupport,
    )
}

#[cfg(test)]
mod tests {
    use super::{EnvelopeRead, SensitiveString};
    #[test]
    fn decoded_envelope_payload_is_zeroized_before_release() {
        let mut envelope = EnvelopeRead {
            version: 1,
            tag: super::EnvelopeTag::ApiKey,
            profile_id: ys_agent_core::ProfileId::new(),
            generation: 1,
            payload: SensitiveString("drop-canary".to_owned()),
        };

        envelope.payload.clear();
        assert!(envelope.payload.0.bytes().all(|byte| byte == 0));
    }
}
