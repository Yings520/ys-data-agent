use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{ArtifactId, CoreError, CoreResult, PrincipalId, RunId, StepId, TaskId, WorkspaceId};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    Public,
    #[default]
    Internal,
    Restricted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RetentionPolicy {
    Session,
    Days { days: u32 },
    Until { expires_at: DateTime<Utc> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    QueryPlan,
    QueryPreflight,
    Query,
    VerificationReport,
    Sql,
    QueryResult,
    ContextEvidence,
    ContextManifest,
    ContextSummary,
    Export,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    Json,
    Csv,
    Markdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactAccessPurpose {
    ModelPreview,
    TuiPreview,
    RuntimeVerification,
    Export,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactAccessContext {
    pub workspace_id: WorkspaceId,
    pub principal_id: PrincipalId,
    pub purpose: ArtifactAccessPurpose,
    pub max_sensitivity: Sensitivity,
}

impl ArtifactAccessContext {
    pub fn allows(
        &self,
        requested_purpose: ArtifactAccessPurpose,
        artifact_sensitivity: Sensitivity,
    ) -> bool {
        let purpose_allowed = requested_purpose == self.purpose
            || requested_purpose == ArtifactAccessPurpose::TuiPreview;

        purpose_allowed && artifact_sensitivity <= self.max_sensitivity
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactMetadata {
    pub id: ArtifactId,
    pub workspace_id: WorkspaceId,
    pub task_id: TaskId,
    pub run_id: RunId,
    pub kind: ArtifactKind,
    pub media_type: String,
    pub content_hash: String,
    pub size_bytes: u64,
    pub storage_uri: String,
    pub sensitivity: Sensitivity,
    pub owner: Option<PrincipalId>,
    pub retention_policy: Option<RetentionPolicy>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub producer_step_id: Option<StepId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub metadata: ArtifactMetadata,
}

impl ArtifactRef {
    pub fn new(metadata: ArtifactMetadata) -> Self {
        Self { metadata }
    }

    pub fn id(&self) -> ArtifactId {
        self.metadata.id
    }
}

#[derive(Debug, Clone)]
pub struct PutArtifact {
    pub workspace_id: WorkspaceId,
    pub task_id: TaskId,
    pub run_id: RunId,
    pub kind: ArtifactKind,
    pub media_type: String,
    pub bytes: Vec<u8>,
    pub sensitivity: Sensitivity,
    pub owner: Option<PrincipalId>,
    pub retention_policy: Option<RetentionPolicy>,
    pub expires_at: Option<DateTime<Utc>>,
    pub producer_step_id: Option<StepId>,
}

#[derive(Debug, Default)]
pub struct ArtifactMetadataBuilder {
    sensitivity: Sensitivity,
    workspace_id: Option<WorkspaceId>,
    task_id: Option<TaskId>,
    run_id: Option<RunId>,
    kind: Option<ArtifactKind>,
    media_type: Option<String>,
    content_hash: Option<String>,
    size_bytes: Option<u64>,
    storage_uri: Option<String>,
    owner: Option<PrincipalId>,
    retention_policy: Option<RetentionPolicy>,
    expires_at: Option<DateTime<Utc>>,
    producer_step_id: Option<StepId>,
}

impl ArtifactMetadata {
    pub fn builder(sensitivity: Sensitivity) -> ArtifactMetadataBuilder {
        ArtifactMetadataBuilder {
            sensitivity,
            ..ArtifactMetadataBuilder::default()
        }
    }
}

impl ArtifactMetadataBuilder {
    pub fn workspace_id(mut self, id: WorkspaceId) -> Self {
        self.workspace_id = Some(id);
        self
    }

    pub fn task_id(mut self, id: TaskId) -> Self {
        self.task_id = Some(id);
        self
    }

    pub fn run_id(mut self, id: RunId) -> Self {
        self.run_id = Some(id);
        self
    }

    pub fn kind(mut self, kind: ArtifactKind) -> Self {
        self.kind = Some(kind);
        self
    }

    pub fn media_type(mut self, media_type: impl Into<String>) -> Self {
        self.media_type = Some(media_type.into());
        self
    }

    pub fn content_hash(mut self, hash: impl Into<String>) -> Self {
        self.content_hash = Some(hash.into());
        self
    }

    pub fn size_bytes(mut self, size: u64) -> Self {
        self.size_bytes = Some(size);
        self
    }

    pub fn storage_uri(mut self, uri: impl Into<String>) -> Self {
        self.storage_uri = Some(uri.into());
        self
    }

    pub fn owner(mut self, owner: PrincipalId) -> Self {
        self.owner = Some(owner);
        self
    }

    pub fn retention_policy(mut self, policy: RetentionPolicy) -> Self {
        self.retention_policy = Some(policy);
        self
    }

    pub fn expires_at(mut self, expires_at: DateTime<Utc>) -> Self {
        self.expires_at = Some(expires_at);
        self
    }

    pub fn producer_step_id(mut self, step_id: StepId) -> Self {
        self.producer_step_id = Some(step_id);
        self
    }

    pub fn build(self) -> CoreResult<ArtifactMetadata> {
        if self.sensitivity == Sensitivity::Restricted {
            if self.retention_policy.is_none() {
                return Err(CoreError::validation(
                    "missing_retention_policy",
                    "restricted artifacts require retention_policy and expires_at",
                ));
            }
            if self.expires_at.is_none() {
                return Err(CoreError::validation(
                    "missing_expiry",
                    "restricted artifacts require retention_policy and expires_at",
                ));
            }

            if self.owner.is_none() {
                return Err(CoreError::validation(
                    "missing_owner",
                    "restricted artifacts require owner",
                ));
            }
        }
        Ok(ArtifactMetadata {
            id: ArtifactId::new(),
            workspace_id: self.workspace_id.unwrap_or_default(),
            task_id: self.task_id.unwrap_or_default(),
            run_id: self.run_id.unwrap_or_default(),
            kind: self.kind.unwrap_or(ArtifactKind::QueryResult),
            media_type: self
                .media_type
                .unwrap_or_else(|| "application/octet-stream".to_owned()),
            content_hash: self
                .content_hash
                .unwrap_or_else(|| "sha256:unset".to_owned()),
            size_bytes: self.size_bytes.unwrap_or(0),
            storage_uri: self
                .storage_uri
                .unwrap_or_else(|| "artifact://pending".to_owned()),
            sensitivity: self.sensitivity,
            owner: self.owner,
            retention_policy: self.retention_policy,
            expires_at: self.expires_at,
            created_at: Utc::now(),
            producer_step_id: self.producer_step_id,
        })
    }
}
