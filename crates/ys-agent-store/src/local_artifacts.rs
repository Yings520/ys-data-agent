use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use sha2::{Digest, Sha256};
use ys_agent_core::{
    ArtifactAccessContext, ArtifactMetadata, ArtifactRef, ArtifactStore, CoreError, CoreResult,
    PutArtifact, RetentionPolicy,
};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

const TEMPORARY_PREFIX: &str = ".ysda-tmp-";
const TEMPORARY_STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone)]
pub struct LocalArtifactStore {
    artifacts: PathBuf,
}

impl LocalArtifactStore {
    pub fn new(root: impl AsRef<Path>) -> CoreResult<Self> {
        let artifacts = root.as_ref().join("artifacts");
        create_owner_only_directory(&artifacts)?;
        cleanup_stale_temporary_files(&artifacts)?;
        Ok(Self { artifacts })
    }

    pub async fn remove_if_expired(
        &self,
        metadata: &ArtifactMetadata,
        now: DateTime<Utc>,
    ) -> CoreResult<bool> {
        let artifacts = self.artifacts.clone();
        let metadata = metadata.clone();
        tokio::task::spawn_blocking(move || remove_if_expired(&artifacts, &metadata, now))
            .await
            .map_err(|error| CoreError::Storage {
                message: format!("artifact worker task failed: {error}"),
            })?
    }
}

#[async_trait]
impl ArtifactStore for LocalArtifactStore {
    async fn put(&self, request: PutArtifact) -> CoreResult<ArtifactMetadata> {
        let artifacts = self.artifacts.clone();
        tokio::task::spawn_blocking(move || put_artifact(&artifacts, request))
            .await
            .map_err(|error| CoreError::Storage {
                message: format!("artifact worker task failed: {error}"),
            })?
    }

    async fn get(
        &self,
        artifact: &ArtifactRef,
        access: &ArtifactAccessContext,
    ) -> CoreResult<Vec<u8>> {
        let artifacts = self.artifacts.clone();
        let metadata = artifact.metadata.clone();
        let access = access.clone();
        tokio::task::spawn_blocking(move || read_artifact(&artifacts, &metadata, &access))
            .await
            .map_err(|error| CoreError::Storage {
                message: format!("artifact worker task failed: {error}"),
            })?
    }
}

fn put_artifact(artififacts: &Path, request: PutArtifact) -> CoreResult<ArtifactMetadata> {
    let hash = hex::encode(Sha256::digest(&request.bytes));
    let directory = artififacts.join(&hash[..2]);
    let destination = directory.join(&hash);
    create_owner_only_directory(&directory)?;

    if !destination.exists() {
        let temporary = directory.join(format!(
            "{TEMPORARY_PREFIX}{}",
            ys_agent_core::ArtifactId::new()
        ));
        write_and_sync(&temporary, &request.bytes)?;

        if destination.exists() {
            fs::remove_file(&temporary).map_err(storage_error)?;
        } else if let Err(error) = fs::rename(&temporary, &destination) {
            let _ = fs::remove_file(&temporary);
            return Err(storage_error(error));
        }
        sync_directory(&directory)?;
    }

    let size_bytes = u64::try_from(request.bytes.len()).map_err(storage_error)?;
    let mut builder = ArtifactMetadata::builder(request.sensitivity)
        .workspace_id(request.workspace_id)
        .task_id(request.task_id)
        .run_id(request.run_id)
        .kind(request.kind)
        .media_type(request.media_type)
        .content_hash(format!("sha256:{hash}"))
        .size_bytes(size_bytes)
        .storage_uri(format!("artifact://sha256/{hash}"));

    if let Some(owner) = request.owner {
        builder = builder.owner(owner);
    }
    if let Some(policy) = request.retention_policy {
        builder = builder.retention_policy(policy);
    }
    if let Some(expires_at) = request.expires_at {
        builder = builder.expires_at(expires_at);
    }
    if let Some(step_id) = request.producer_step_id {
        builder = builder.producer_step_id(step_id);
    }
    builder.build()
}

fn read_artifact(
    artifacts: &Path,
    metadata: &ArtifactMetadata,
    access: &ArtifactAccessContext,
) -> CoreResult<Vec<u8>> {
    if metadata.workspace_id != access.workspace_id {
        return Err(CoreError::ArtifactAccessDenied {
            reason: "artifact belongs to another workspace".to_owned(),
        });
    }
    if metadata
        .owner
        .is_some_and(|owner| owner != access.principal_id)
    {
        return Err(CoreError::ArtifactAccessDenied {
            reason: "artifact belongs to another principal".to_owned(),
        });
    }
    if !access.allows(access.purpose, metadata.sensitivity) {
        return Err(CoreError::ArtifactAccessDenied {
            reason: "purpose or sensitivity is not allowed".to_owned(),
        });
    }
    if expires_at(metadata).is_some_and(|expiry| expiry <= Utc::now()) {
        return Err(CoreError::ArtifactAccessDenied {
            reason: "artifact has expired".to_owned(),
        });
    }

    let hash = metadata_hash(metadata)?;
    let path = content_path(artifacts, hash);
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(CoreError::CorruptArtifact {
                artifact_id: metadata.id.to_string(),
                reason: "metadata references missing content".to_owned(),
            });
        }
        Err(error) => return Err(storage_error(error)),
    };

    let actual = hex::encode(Sha256::digest(&bytes));
    if actual != hash {
        return Err(CoreError::CorruptArtifact {
            artifact_id: metadata.id.to_string(),
            reason: "stored bytes do not match content_hash".to_owned(),
        });
    }
    Ok(bytes)
}

fn remove_if_expired(
    artifacts: &Path,
    metadata: &ArtifactMetadata,
    now: DateTime<Utc>,
) -> CoreResult<bool> {
    let Some(expiry) = expires_at(metadata) else {
        return Ok(false);
    };
    if expiry > now {
        return Ok(false);
    }

    let hash = metadata_hash(metadata)?;
    let path = content_path(artifacts, hash);
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(storage_error(error)),
    }
}

fn expires_at(metadata: &ArtifactMetadata) -> Option<DateTime<Utc>> {
    metadata
        .expires_at
        .or_else(|| match &metadata.retention_policy {
            Some(RetentionPolicy::Days { days }) => metadata
                .created_at
                .checked_add_signed(TimeDelta::days(i64::from(*days))),
            Some(RetentionPolicy::Until { expires_at }) => Some(*expires_at),
            Some(RetentionPolicy::Session) | None => None,
        })
}

fn metadata_hash(metadata: &ArtifactMetadata) -> CoreResult<&str> {
    let Some(hash) = metadata.content_hash.strip_prefix("sha256:") else {
        return Err(CoreError::CorruptArtifact {
            artifact_id: metadata.id.to_string(),
            reason: "content_hash does not use sha256".to_owned(),
        });
    };
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CoreError::CorruptArtifact {
            artifact_id: metadata.id.to_string(),
            reason: "content_hash is not a valid SHA-256 digest".to_owned(),
        });
    }
    Ok(hash)
}

fn content_path(artifacts: &Path, hash: &str) -> PathBuf {
    artifacts.join(&hash[..2]).join(hash)
}

fn write_and_sync(path: &Path, bytes: &[u8]) -> CoreResult<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);

    let mut file = options.open(path).map_err(storage_error)?;
    file.write_all(bytes).map_err(storage_error)?;
    file.sync_all().map_err(storage_error)
}

fn create_owner_only_directory(path: &Path) -> CoreResult<()> {
    fs::create_dir_all(path).map_err(storage_error)?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(storage_error)?;
    Ok(())
}

fn sync_directory(path: &Path) -> CoreResult<()> {
    #[cfg(unix)]
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(storage_error)?;
    Ok(())
}

fn cleanup_stale_temporary_files(artifacts: &Path) -> CoreResult<()> {
    for entry in fs::read_dir(artifacts).map_err(storage_error)? {
        let entry = entry.map_err(storage_error)?;
        if !entry.file_type().map_err(storage_error)?.is_dir() {
            continue;
        }
        for candidate in fs::read_dir(entry.path()).map_err(storage_error)? {
            let candidate = candidate.map_err(storage_error)?;
            if candidate.file_type().map_err(storage_error)?.is_file()
                && candidate
                    .file_name()
                    .to_string_lossy()
                    .starts_with(TEMPORARY_PREFIX)
                && is_stale_temporary_file(&candidate.path())?
            {
                fs::remove_file(candidate.path()).map_err(storage_error)?;
            }
        }
    }
    Ok(())
}

fn is_stale_temporary_file(path: &Path) -> CoreResult<bool> {
    let modified = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map_err(storage_error)?;
    match SystemTime::now().duration_since(modified) {
        Ok(age) => Ok(age >= TEMPORARY_STALE_AFTER),
        Err(_) => Ok(false),
    }
}

fn storage_error(error: impl std::fmt::Display) -> CoreError {
    CoreError::Storage {
        message: error.to_string(),
    }
}
