//! Datasource-only vault namespace. It does not promise OS Keychain-level protection.
use async_trait::async_trait;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use ys_agent_core::{
    DatasourceSecretRef, DatasourceVault, DsError, DsErrorCode, DsRemediation, DsResult,
    ProtectionStatus, SecretLease, SecretValue,
};

#[derive(Clone)]
pub struct LocalEncryptedDatasourceVault {
    root: Arc<PathBuf>,
}
impl LocalEncryptedDatasourceVault {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: Arc::new(root.as_ref().to_path_buf()),
        }
    }
}
impl std::fmt::Debug for LocalEncryptedDatasourceVault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalEncryptedDatasourceVault")
            .finish_non_exhaustive()
    }
}
fn error(code: DsErrorCode) -> DsError {
    DsError {
        code,
        field: None,
        remediation: DsRemediation::RepairProtection,
        operation_id: None,
    }
}
async fn blocking<T: Send + 'static>(
    operation: impl FnOnce() -> DsResult<T> + Send + 'static,
) -> DsResult<T> {
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|_| error(DsErrorCode::Storage))?
}

#[cfg(unix)]
mod protected {
    use super::*;
    use crate::credential::encrypted::{self, KEY_LENGTH, private_files::PrivateDirectory};
    use ring::rand::{SecureRandom, SystemRandom};
    use std::io;
    use zeroize::Zeroizing;
    const KEY: &str = "datasource-credentials.key";
    const ENTRIES: &str = "datasource-credentials";
    fn io_error(e: io::Error) -> DsError {
        error(match e.kind() {
            io::ErrorKind::AlreadyExists => DsErrorCode::Conflict,
            io::ErrorKind::NotFound => DsErrorCode::CredentialMissing,
            _ => DsErrorCode::ProtectionUnavailable,
        })
    }
    fn key(directory: &PrivateDirectory, create: bool) -> DsResult<Zeroizing<[u8; KEY_LENGTH]>> {
        let bytes = match directory.read(KEY, KEY_LENGTH as u64) {
            Ok(bytes) => bytes,
            Err(e) if create && e.kind() == io::ErrorKind::NotFound => {
                let mut key = Zeroizing::new([0; KEY_LENGTH]);
                SystemRandom::new()
                    .fill(&mut *key)
                    .map_err(|_| error(DsErrorCode::ProtectionUnavailable))?;
                match directory.write_new(KEY, &key[..]) {
                    Ok(()) => return Ok(key),
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                        directory.read(KEY, KEY_LENGTH as u64).map_err(io_error)?
                    }
                    Err(e) => return Err(io_error(e)),
                }
            }
            Err(_) => return Err(error(DsErrorCode::ProtectionUnavailable)),
        };
        if bytes.len() != KEY_LENGTH {
            return Err(error(DsErrorCode::ProtectionUnavailable));
        }
        let mut key = Zeroizing::new([0; KEY_LENGTH]);
        key.copy_from_slice(&bytes);
        Ok(key)
    }
    fn name(r: DatasourceSecretRef) -> String {
        format!(
            "{}-{}-{}.credential",
            r.workspace_id(),
            r.profile_id(),
            r.generation()
        )
    }
    fn aad(r: DatasourceSecretRef) -> String {
        format!(
            "ysda-datasource-credential-v1:{}:{}:{}",
            r.workspace_id(),
            r.profile_id(),
            r.generation()
        )
    }
    pub(super) fn protection(path: &Path) -> DsResult<ProtectionStatus> {
        let result = PrivateDirectory::open(path).and_then(|d| d.child(ENTRIES));
        Ok(if result.is_ok() {
            ProtectionStatus::OwnerOnlyEncryptedFile
        } else {
            ProtectionStatus::Unavailable
        })
    }
    pub(super) fn write(
        path: &Path,
        reference: DatasourceSecretRef,
        value: SecretValue,
    ) -> DsResult<()> {
        let root = PrivateDirectory::open(path).map_err(io_error)?;
        let entries = root.child(ENTRIES).map_err(io_error)?;
        let key = key(&root, true)?;
        let encrypted = value
            .with_exposed(|s| encrypted::encrypt(&key, aad(reference).as_bytes(), s))
            .map_err(|_| error(DsErrorCode::ProtectionUnavailable))?;
        entries
            .write_new(&name(reference), &encrypted)
            .map_err(io_error)
    }
    pub(super) fn read(path: &Path, reference: DatasourceSecretRef) -> DsResult<SecretLease> {
        let root = PrivateDirectory::open(path).map_err(io_error)?;
        let key = key(&root, false)?;
        let bytes = root
            .child(ENTRIES)
            .map_err(io_error)?
            .read(&name(reference), 1024 * 1024)
            .map_err(io_error)?;
        let value = encrypted::decrypt(&key, aad(reference).as_bytes(), &bytes)
            .map(SecretValue::from_utf8)
            .map_err(|_| error(DsErrorCode::ProtectionUnavailable))?;
        Ok(SecretLease { reference, value })
    }
    pub(super) fn remove(path: &Path, reference: DatasourceSecretRef) -> DsResult<()> {
        PrivateDirectory::open(path)
            .map_err(io_error)?
            .child(ENTRIES)
            .map_err(io_error)?
            .remove(&name(reference))
            .map_err(io_error)
    }
}

#[cfg(not(unix))]
mod protected {
    use super::*;
    pub(super) fn protection(_: &Path) -> DsResult<ProtectionStatus> {
        Ok(ProtectionStatus::Unavailable)
    }
    pub(super) fn write(_: &Path, _: DatasourceSecretRef, _: SecretValue) -> DsResult<()> {
        Err(error(DsErrorCode::ProtectionUnavailable))
    }
    pub(super) fn read(_: &Path, _: DatasourceSecretRef) -> DsResult<SecretLease> {
        Err(error(DsErrorCode::ProtectionUnavailable))
    }
    pub(super) fn remove(_: &Path, _: DatasourceSecretRef) -> DsResult<()> {
        Err(error(DsErrorCode::ProtectionUnavailable))
    }
}

#[async_trait]
impl DatasourceVault for LocalEncryptedDatasourceVault {
    async fn protection(&self) -> DsResult<ProtectionStatus> {
        let root = self.root.clone();
        blocking(move || protected::protection(&root)).await
    }
    async fn write(&self, reference: DatasourceSecretRef, value: SecretValue) -> DsResult<()> {
        let root = self.root.clone();
        blocking(move || protected::write(&root, reference, value)).await
    }
    async fn read(&self, reference: DatasourceSecretRef) -> DsResult<SecretLease> {
        let root = self.root.clone();
        blocking(move || protected::read(&root, reference)).await
    }
    async fn remove(&self, reference: DatasourceSecretRef) -> DsResult<()> {
        let root = self.root.clone();
        blocking(move || protected::remove(&root, reference)).await
    }
}
