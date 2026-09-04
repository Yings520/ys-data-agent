//! Explicit in-memory credential-vault test double.

use std::{
    collections::BTreeMap,
    fmt,
    sync::{Arc, Mutex, MutexGuard},
};

use async_trait::async_trait;
use ys_agent_core::{
    CredentialLease, CredentialProtectionStatus, CredentialVault, CredentialViewStatus,
    ProtectedCredentialWrite, ProviderCredentialReference, ProviderErrorCode, ProviderField,
    ProviderManagementError, ProviderRemediation, ProviderResult, SecretValue,
};
use zeroize::{Zeroize, Zeroizing};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InMemoryVaultOperation {
    Write,
    Read,
    Delete,
}

#[derive(Clone)]
pub struct InMemoryCredentialVault {
    state: Arc<Mutex<InMemoryState>>,
    protection: CredentialProtectionStatus,
}

impl InMemoryCredentialVault {
    pub fn new() -> Self {
        Self::with_protection(CredentialProtectionStatus::ConfirmedNative)
    }

    pub fn with_protection(protection: CredentialProtectionStatus) -> Self {
        Self {
            state: Arc::new(Mutex::new(InMemoryState::default())),
            protection,
        }
    }

    pub fn restart(&self) -> Self {
        self.clone()
    }

    pub fn fail_next(&self, operation: InMemoryVaultOperation) {
        self.lock().fail_next = Some(operation);
    }

    pub fn stored_accounts(&self) -> Vec<String> {
        self.lock().entries.keys().cloned().collect()
    }

    fn lock(&self) -> MutexGuard<'_, InMemoryState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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
            .field("entry_count", &self.lock().entries.len())
            .finish()
    }
}

#[async_trait]
impl CredentialVault for InMemoryCredentialVault {
    async fn protection_status(&self) -> ProviderResult<CredentialProtectionStatus> {
        Ok(self.protection)
    }

    async fn credential_status(
        &self,
        reference: ProviderCredentialReference,
    ) -> ProviderResult<CredentialViewStatus> {
        validate_reference(&reference)?;
        if !self.protection.is_confirmed() {
            return Ok(CredentialViewStatus::ProtectionUnavailable);
        }
        let mut state = self.lock();
        if take_fault(&mut state, InMemoryVaultOperation::Read) {
            return Err(protection_unavailable());
        }
        match state.entries.get(&account(&reference)) {
            Some(entry) if entry.kind == reference.generation.kind() => {
                Ok(CredentialViewStatus::Saved)
            }
            Some(_) => Err(storage_conflict()),
            None => Ok(CredentialViewStatus::Missing),
        }
    }

    async fn write_generation(&self, input: ProtectedCredentialWrite) -> ProviderResult<()> {
        validate_reference(&input.reference)?;
        if !self.protection.is_confirmed() {
            return Err(protection_unavailable());
        }
        let mut state = self.lock();
        if take_fault(&mut state, InMemoryVaultOperation::Write) {
            return Err(protection_unavailable());
        }
        let account = account(&input.reference);
        if state.entries.contains_key(&account) {
            return Err(storage_conflict());
        }
        let value = input
            .secret
            .with_exposed(|secret| secret.as_bytes().to_vec());
        state.entries.insert(
            account,
            StoredCredential {
                kind: input.reference.generation.kind(),
                value,
            },
        );
        Ok(())
    }

    async fn read_generation(
        &self,
        reference: ProviderCredentialReference,
    ) -> ProviderResult<CredentialLease> {
        validate_reference(&reference)?;
        if !self.protection.is_confirmed() {
            return Err(protection_unavailable());
        }
        let mut state = self.lock();
        if take_fault(&mut state, InMemoryVaultOperation::Read) {
            return Err(protection_unavailable());
        }
        let entry = state
            .entries
            .get(&account(&reference))
            .ok_or_else(credential_missing)?;
        if entry.kind != reference.generation.kind() {
            return Err(storage_conflict());
        }
        let bytes = Zeroizing::new(entry.value.clone());
        let secret = String::from_utf8(bytes.to_vec()).map_err(|_| storage_conflict())?;
        Ok(CredentialLease::new(SecretValue::from_utf8(secret)))
    }

    async fn delete_generation(
        &self,
        reference: ProviderCredentialReference,
    ) -> ProviderResult<()> {
        validate_reference(&reference)?;
        if !self.protection.is_confirmed() {
            return Err(protection_unavailable());
        }
        let mut state = self.lock();
        if take_fault(&mut state, InMemoryVaultOperation::Delete) {
            return Err(protection_unavailable());
        }
        if let Some(mut entry) = state.entries.remove(&account(&reference)) {
            entry.value.zeroize();
        }
        Ok(())
    }
}

#[derive(Default)]
struct InMemoryState {
    entries: BTreeMap<String, StoredCredential>,
    fail_next: Option<InMemoryVaultOperation>,
}

struct StoredCredential {
    kind: ys_agent_core::CredentialKind,
    value: Vec<u8>,
}

impl Drop for InMemoryState {
    fn drop(&mut self) {
        for entry in self.entries.values_mut() {
            entry.value.zeroize();
        }
    }
}

fn validate_reference(reference: &ProviderCredentialReference) -> ProviderResult<()> {
    if reference.profile_id != reference.generation.profile_id() {
        return Err(storage_conflict());
    }
    Ok(())
}

fn account(reference: &ProviderCredentialReference) -> String {
    format!("{}:{}", reference.profile_id, reference.generation.number())
}

fn take_fault(state: &mut InMemoryState, operation: InMemoryVaultOperation) -> bool {
    if state.fail_next == Some(operation) {
        state.fail_next = None;
        true
    } else {
        false
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
