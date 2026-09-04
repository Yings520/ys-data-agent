//! Workspace-local encrypted credential vault.
//!
//! This adapter deliberately has no OS credential-store integration. It keeps the encryption
//! key and encrypted immutable credential generations under the workspace directory with
//! owner-only permissions, so normal startup and TUI refreshes never invoke platform prompts.

use std::{
    fmt,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use ring::{
    aead::{self, Aad, LessSafeKey, UnboundKey},
    rand::{SecureRandom, SystemRandom},
};
use ys_agent_core::{
    CredentialLease, CredentialProtectionStatus, CredentialVault, CredentialViewStatus,
    ProtectedCredentialWrite, ProviderCredentialReference, ProviderErrorCode, ProviderField,
    ProviderManagementError, ProviderRemediation, ProviderResult, SecretValue,
};
use zeroize::{Zeroize, Zeroizing};

const KEY_FILE: &str = "provider-credentials.key";
const ENTRIES_DIRECTORY: &str = "provider-credentials";
const ENVELOPE_VERSION: u8 = 1;
const KEY_LENGTH: usize = 32;
const NONCE_LENGTH: usize = 12;

/// The production credential vault. It never constructs or contacts an OS credential entry.
#[derive(Clone)]
pub struct LocalEncryptedCredentialVault {
    root: Arc<PathBuf>,
    mutation_lock: Arc<Mutex<()>>,
}

impl LocalEncryptedCredentialVault {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: Arc::new(root.as_ref().to_path_buf()),
            mutation_lock: Arc::new(Mutex::new(())),
        }
    }
}

impl fmt::Debug for LocalEncryptedCredentialVault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalEncryptedCredentialVault")
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl CredentialVault for LocalEncryptedCredentialVault {
    async fn protection_status(&self) -> ProviderResult<CredentialProtectionStatus> {
        let root = self.root.clone();
        let status = run_blocking(move || {
            if ensure_private_directory(root.as_ref()).is_ok() {
                CredentialProtectionStatus::ConfirmedLocal
            } else {
                CredentialProtectionStatus::Unavailable
            }
        })
        .await?;
        Ok(status)
    }

    async fn credential_status(
        &self,
        reference: ProviderCredentialReference,
    ) -> ProviderResult<CredentialViewStatus> {
        validate_reference(&reference)?;
        let root = self.root.clone();
        run_blocking(move || {
            let path = credential_path(root.as_ref(), &reference);
            let encrypted = match fs::read(path) {
                Ok(value) => value,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Ok(CredentialViewStatus::Missing);
                }
                Err(_) => return Ok(CredentialViewStatus::ProtectionUnavailable),
            };
            let key = match read_existing_key(root.as_ref()) {
                Ok(key) => key,
                Err(_) => return Ok(CredentialViewStatus::ProtectionUnavailable),
            };
            match decrypt_generation(&key, &reference, encrypted) {
                Ok(_) => Ok(CredentialViewStatus::Saved),
                Err(error) if error.code() == "provider.storage.conflict" => Err(error),
                Err(_) => Ok(CredentialViewStatus::ProtectionUnavailable),
            }
        })
        .await?
    }

    async fn write_generation(&self, input: ProtectedCredentialWrite) -> ProviderResult<()> {
        validate_reference(&input.reference)?;
        let root = self.root.clone();
        let lock = self.mutation_lock.clone();
        run_blocking(move || {
            let _guard = lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let key = read_or_create_key(root.as_ref())?;
            let encrypted = input
                .secret
                .with_exposed(|secret| encrypt_generation(&key, &input.reference, secret))?;
            write_new_generation(root.as_ref(), &input.reference, &encrypted)
        })
        .await?
    }

    async fn read_generation(
        &self,
        reference: ProviderCredentialReference,
    ) -> ProviderResult<CredentialLease> {
        validate_reference(&reference)?;
        let root = self.root.clone();
        run_blocking(move || {
            let encrypted =
                fs::read(credential_path(root.as_ref(), &reference)).map_err(|error| {
                    if error.kind() == io::ErrorKind::NotFound {
                        credential_missing()
                    } else {
                        protection_unavailable()
                    }
                })?;
            let key = read_existing_key(root.as_ref())?;
            decrypt_generation(&key, &reference, encrypted).map(CredentialLease::new)
        })
        .await?
    }

    async fn delete_generation(
        &self,
        reference: ProviderCredentialReference,
    ) -> ProviderResult<()> {
        validate_reference(&reference)?;
        let root = self.root.clone();
        let lock = self.mutation_lock.clone();
        run_blocking(move || {
            let _guard = lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match fs::remove_file(credential_path(root.as_ref(), &reference)) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(_) => Err(protection_unavailable()),
            }
        })
        .await?
    }
}

async fn run_blocking<T>(operation: impl FnOnce() -> T + Send + 'static) -> ProviderResult<T>
where
    T: Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|_| internal_error())
}

fn validate_reference(reference: &ProviderCredentialReference) -> ProviderResult<()> {
    if reference.profile_id != reference.generation.profile_id() {
        return Err(storage_conflict());
    }
    Ok(())
}

fn credential_path(root: &Path, reference: &ProviderCredentialReference) -> PathBuf {
    root.join(ENTRIES_DIRECTORY).join(format!(
        "{}-{}.credential",
        reference.profile_id,
        reference.generation.number()
    ))
}

fn key_path(root: &Path) -> PathBuf {
    root.join(KEY_FILE)
}

fn ensure_private_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    enforce_private_directory_permissions(path)
}

fn read_or_create_key(root: &Path) -> ProviderResult<Zeroizing<[u8; KEY_LENGTH]>> {
    ensure_private_directory(root).map_err(|_| protection_unavailable())?;
    let path = key_path(root);
    match read_key(&path) {
        Ok(key) => Ok(key),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut key = Zeroizing::new([0_u8; KEY_LENGTH]);
            SystemRandom::new()
                .fill(&mut *key)
                .map_err(|_| internal_error())?;
            match write_new_private_file(&path, &key[..]) {
                Ok(()) => Ok(key),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    read_existing_key(root)
                }
                Err(_) => Err(protection_unavailable()),
            }
        }
        Err(_) => Err(protection_unavailable()),
    }
}

fn read_existing_key(root: &Path) -> ProviderResult<Zeroizing<[u8; KEY_LENGTH]>> {
    read_key(&key_path(root)).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            protection_unavailable()
        } else {
            storage_conflict()
        }
    })
}

fn read_key(path: &Path) -> io::Result<Zeroizing<[u8; KEY_LENGTH]>> {
    let mut bytes = fs::read(path)?;
    enforce_private_file_permissions(path)?;
    if bytes.len() != KEY_LENGTH {
        bytes.zeroize();
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid local key length",
        ));
    }
    let mut key = Zeroizing::new([0_u8; KEY_LENGTH]);
    key.copy_from_slice(&bytes);
    bytes.zeroize();
    Ok(key)
}

fn write_new_generation(
    root: &Path,
    reference: &ProviderCredentialReference,
    encrypted: &[u8],
) -> ProviderResult<()> {
    let entries = root.join(ENTRIES_DIRECTORY);
    ensure_private_directory(&entries).map_err(|_| protection_unavailable())?;
    let path = credential_path(root, reference);
    match write_new_private_file(&path, encrypted) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Err(storage_conflict()),
        Err(_) => Err(protection_unavailable()),
    }
}

fn write_new_private_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = open_new_private_file(path)?;
    let result = file.write_all(bytes).and_then(|_| file.sync_all());
    drop(file);
    if let Err(error) = result {
        let _ = fs::remove_file(path);
        return Err(error);
    }
    enforce_private_file_permissions(path)
}

fn open_new_private_file(path: &Path) -> io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

#[cfg(unix)]
fn enforce_private_directory_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn enforce_private_directory_permissions(_: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn enforce_private_file_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn enforce_private_file_permissions(_: &Path) -> io::Result<()> {
    Ok(())
}

fn encrypt_generation(
    key: &[u8; KEY_LENGTH],
    reference: &ProviderCredentialReference,
    secret: &str,
) -> ProviderResult<Vec<u8>> {
    if secret.is_empty() {
        return Err(storage_conflict());
    }
    let cipher = cipher(key)?;
    let mut nonce_bytes = [0_u8; NONCE_LENGTH];
    SystemRandom::new()
        .fill(&mut nonce_bytes)
        .map_err(|_| internal_error())?;
    let aad = associated_data(reference);
    let mut ciphertext = secret.as_bytes().to_vec();
    cipher
        .seal_in_place_append_tag(
            aead::Nonce::assume_unique_for_key(nonce_bytes),
            Aad::from(aad.as_slice()),
            &mut ciphertext,
        )
        .map_err(|_| internal_error())?;
    let mut encoded = Vec::with_capacity(1 + NONCE_LENGTH + ciphertext.len());
    encoded.push(ENVELOPE_VERSION);
    encoded.extend_from_slice(&nonce_bytes);
    encoded.extend_from_slice(&ciphertext);
    ciphertext.zeroize();
    Ok(encoded)
}

fn decrypt_generation(
    key: &[u8; KEY_LENGTH],
    reference: &ProviderCredentialReference,
    encrypted: Vec<u8>,
) -> ProviderResult<SecretValue> {
    if encrypted.len() <= 1 + NONCE_LENGTH || encrypted[0] != ENVELOPE_VERSION {
        return Err(storage_conflict());
    }
    let cipher = cipher(key)?;
    let mut nonce_bytes = [0_u8; NONCE_LENGTH];
    nonce_bytes.copy_from_slice(&encrypted[1..=NONCE_LENGTH]);
    let mut ciphertext = Zeroizing::new(encrypted[(1 + NONCE_LENGTH)..].to_vec());
    let aad = associated_data(reference);
    let plaintext_length = cipher
        .open_in_place(
            aead::Nonce::assume_unique_for_key(nonce_bytes),
            Aad::from(aad.as_slice()),
            &mut ciphertext,
        )
        .map_err(|_| storage_conflict())?
        .len();
    let mut plaintext = ciphertext[..plaintext_length].to_vec();
    ciphertext.zeroize();
    let secret = String::from_utf8(std::mem::take(&mut plaintext)).map_err(|error| {
        let mut malformed = error.into_bytes();
        malformed.zeroize();
        storage_conflict()
    })?;
    Ok(SecretValue::from_utf8(secret))
}

fn cipher(key: &[u8; KEY_LENGTH]) -> ProviderResult<LessSafeKey> {
    UnboundKey::new(&aead::AES_256_GCM, key)
        .map(LessSafeKey::new)
        .map_err(|_| internal_error())
}

fn associated_data(reference: &ProviderCredentialReference) -> Vec<u8> {
    let kind = match reference.generation.kind() {
        ys_agent_core::CredentialKind::ApiKey => "api_key",
        ys_agent_core::CredentialKind::OAuthConnection => "oauth_connection",
    };
    format!(
        "ysda-local-credential-v1:{}:{}:{kind}",
        reference.profile_id,
        reference.generation.number()
    )
    .into_bytes()
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
