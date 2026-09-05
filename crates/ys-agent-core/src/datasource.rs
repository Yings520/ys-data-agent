use std::{collections::BTreeMap, num::NonZeroU64};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

use crate::{CoreError, CoreResult, ProfileId, SecretValue, SourceId, WorkspaceId};

macro_rules! adapter_identity {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> CoreResult<Self> {
                let value = value.into();
                if value.is_empty()
                    || !value
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-+".contains(&b))
                {
                    return Err(CoreError::validation(
                        "invalid_adapter_identity",
                        "adapter identity must be a nonempty versioned identifier",
                    ));
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = CoreError;
            fn try_from(value: String) -> CoreResult<Self> {
                Self::new(value)
            }
        }

        impl TryFrom<&str> for $name {
            type Error = CoreError;
            fn try_from(value: &str) -> CoreResult<Self> {
                Self::new(value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

adapter_identity!(AdapterId);
adapter_identity!(AdapterVersion);

/// Physical configuration remains confined to the protected Profile store. Run bindings use
/// its digest and the exact revision instead of serializing filesystem paths.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DatabaseContext {
    Unconfigured,
    Database {
        catalog: Option<String>,
        database: String,
        schema: String,
    },
    File {
        canonical_path: std::path::PathBuf,
    },
}

impl std::fmt::Debug for DatabaseContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unconfigured => "DatabaseContext::Unconfigured",
            Self::Database { .. } => "DatabaseContext::Database([redacted])",
            Self::File { .. } => "DatabaseContext::File([redacted])",
        })
    }
}

impl DatabaseContext {
    pub fn validate(&self) -> CoreResult<()> {
        let valid_identifier = |s: &str| {
            !s.trim().is_empty() && !s.chars().any(char::is_control) && !s.contains("://")
        };
        let valid = match self {
            Self::Unconfigured => true,
            Self::Database {
                catalog,
                database,
                schema,
            } => {
                catalog.as_deref().is_none_or(valid_identifier)
                    && valid_identifier(database)
                    && valid_identifier(schema)
            }
            Self::File { canonical_path } => {
                canonical_path.is_absolute()
                    && canonical_path.components().all(|c| {
                        !matches!(
                            c,
                            std::path::Component::ParentDir | std::path::Component::CurDir
                        )
                    })
                    && canonical_path.to_str().is_some_and(valid_identifier)
            }
        };
        if valid {
            Ok(())
        } else {
            Err(CoreError::validation(
                "invalid_database_context",
                "database context is invalid",
            ))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DatasourceRevisionId {
    pub workspace_id: WorkspaceId,
    pub profile_id: ProfileId,
    pub revision: NonZeroU64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasourceRevisionInput {
    pub schema_version: u32,
    pub workspace_id: WorkspaceId,
    pub profile_id: ProfileId,
    pub revision: u64,
    pub adapter_id: AdapterId,
    pub adapter_version: AdapterVersion,
    pub config_version: u32,
    pub source_id: Option<SourceId>,
    pub fields: BTreeMap<FieldId, FieldValue>,
    pub context: DatabaseContext,
    pub credential: Option<DatasourceSecretRef>,
}

/// Immutable saved content. Validation and selections are independent facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "DatasourceRevisionInput", into = "DatasourceRevisionInput")]
pub struct DatasourceRevision(DatasourceRevisionInput);

impl DatasourceRevision {
    pub fn new(input: DatasourceRevisionInput) -> CoreResult<Self> {
        if input.schema_version != 1 {
            return Err(CoreError::UnsupportedSchemaVersion {
                version: input.schema_version,
            });
        }
        if input.revision == 0 || input.config_version == 0 {
            return Err(CoreError::validation(
                "invalid_datasource_revision",
                "revision and configuration versions start at one",
            ));
        }
        input.context.validate()?;
        if input.credential.is_some_and(|c| {
            c.workspace_id() != input.workspace_id || c.profile_id() != input.profile_id
        }) {
            return Err(CoreError::validation(
                "datasource_credential_mismatch",
                "credential must belong to the same Workspace and Profile",
            ));
        }
        if input
            .source_id
            .as_ref()
            .is_some_and(|s| s.as_str().trim().is_empty())
        {
            return Err(CoreError::validation(
                "invalid_datasource_source",
                "source identity must be nonempty",
            ));
        }
        Ok(Self(input))
    }

    pub fn identity(&self) -> DatasourceRevisionId {
        DatasourceRevisionId {
            workspace_id: self.0.workspace_id,
            profile_id: self.0.profile_id,
            revision: NonZeroU64::new(self.0.revision).expect("validated revision"),
        }
    }

    pub fn number(&self) -> u64 {
        self.0.revision
    }
    pub fn input(&self) -> &DatasourceRevisionInput {
        &self.0
    }

    pub fn ensure_config_contract(
        &self,
        adapter: &AdapterId,
        version: &AdapterVersion,
        config_version: u32,
    ) -> CoreResult<()> {
        if &self.0.adapter_id != adapter
            || &self.0.adapter_version != version
            || self.0.config_version != config_version
        {
            return Err(CoreError::validation(
                "datasource_config_incompatible",
                "saved configuration does not match the registered contract",
            ));
        }
        Ok(())
    }
}

impl TryFrom<DatasourceRevisionInput> for DatasourceRevision {
    type Error = CoreError;
    fn try_from(input: DatasourceRevisionInput) -> CoreResult<Self> {
        Self::new(input)
    }
}

impl From<DatasourceRevision> for DatasourceRevisionInput {
    fn from(revision: DatasourceRevision) -> Self {
        revision.0
    }
}

/// SHA-256 over canonical non-secret configuration or evidence, never secret bytes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DatasourceDigest([u8; 32]);

impl DatasourceDigest {
    pub fn of(value: &impl Serialize) -> CoreResult<Self> {
        let bytes = serde_json::to_vec(value).map_err(|_| {
            CoreError::validation(
                "invalid_datasource_evidence",
                "datasource evidence could not be encoded",
            )
        })?;
        Ok(Self(Sha256::digest(bytes).into()))
    }

    pub fn hex(&self) -> String {
        hex::encode(self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasourceValidationInputs {
    schema_version: u32,
    revision: DatasourceRevisionId,
    adapter_id: AdapterId,
    adapter_version: AdapterVersion,
    config_version: u32,
    config_digest: DatasourceDigest,
    credential: Option<DatasourceSecretRef>,
    context_digest: DatasourceDigest,
    capability: crate::CapabilityDescriptor,
    policy_digest: DatasourceDigest,
}

impl DatasourceValidationInputs {
    pub fn new(
        revision: &DatasourceRevision,
        capability: &crate::CapabilityDescriptor,
        policy_digest: DatasourceDigest,
    ) -> CoreResult<Self> {
        let input = revision.input();
        if input.source_id.as_ref() != Some(&capability.source_id)
            || !capability.supports_governed_query()
            || matches!(input.context, DatabaseContext::Unconfigured)
        {
            return Err(CoreError::validation(
                "datasource_not_ready",
                "complete source context and governed capabilities are required",
            ));
        }
        Ok(Self {
            schema_version: 1,
            revision: revision.identity(),
            adapter_id: input.adapter_id.clone(),
            adapter_version: input.adapter_version.clone(),
            config_version: input.config_version,
            config_digest: DatasourceDigest::of(&input.fields)?,
            credential: input.credential,
            context_digest: DatasourceDigest::of(&input.context)?,
            capability: capability.clone(),
            policy_digest,
        })
    }

    pub fn revision(&self) -> DatasourceRevisionId {
        self.revision
    }
    pub fn context_digest(&self) -> &DatasourceDigest {
        &self.context_digest
    }
    pub fn policy_digest(&self) -> &DatasourceDigest {
        &self.policy_digest
    }
    pub fn capability(&self) -> &crate::CapabilityDescriptor {
        &self.capability
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeEvidence {
    pub authenticated: bool,
    pub target_verified: bool,
    pub read_only_verified: bool,
    pub least_privilege_verified: bool,
    pub capabilities_verified: bool,
}

impl ProbeEvidence {
    pub fn passed(self) -> bool {
        self.authenticated
            && self.target_verified
            && self.read_only_verified
            && self.least_privilege_verified
            && self.capabilities_verified
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidationEvidence {
    schema_version: u32,
    id: crate::ValidationId,
    inputs: DatasourceValidationInputs,
    engine_version: AdapterVersion,
    probe: ProbeEvidence,
    validated_at: chrono::DateTime<chrono::Utc>,
}

impl ValidationEvidence {
    /// Construct only after local configuration, registered version and Policy checks passed.
    pub fn new(
        inputs: DatasourceValidationInputs,
        engine_version: AdapterVersion,
        probe: ProbeEvidence,
        validated_at: chrono::DateTime<chrono::Utc>,
    ) -> CoreResult<Self> {
        if !probe.passed()
            || inputs.schema_version != 1
            || !inputs.capability.supports_governed_query()
        {
            return Err(CoreError::validation(
                "datasource_probe_failed",
                "all connection and read-only evidence must be proven",
            ));
        }
        Ok(Self {
            schema_version: 1,
            id: crate::ValidationId::new(),
            inputs,
            engine_version,
            probe,
            validated_at,
        })
    }

    pub fn matches(&self, inputs: &DatasourceValidationInputs) -> bool {
        self.schema_version == 1
            && inputs.schema_version == 1
            && self.probe.passed()
            && inputs.capability.supports_governed_query()
            && &self.inputs == inputs
    }

    pub fn id(&self) -> crate::ValidationId {
        self.id
    }
    pub fn inputs(&self) -> &DatasourceValidationInputs {
        &self.inputs
    }
    pub fn validated_at(&self) -> chrono::DateTime<chrono::Utc> {
        self.validated_at
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasourceScope {
    pub workspace_id: WorkspaceId,
    pub session_id: crate::SessionId,
}

/// Safe, immutable evidence for a single Run. Physical file paths are resolved from the exact
/// retained Profile revision, never copied into events or artifacts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunDatasourceBinding {
    schema_version: u32,
    run_id: crate::RunId,
    scope: DatasourceScope,
    selection_version: u64,
    evidence: ValidationEvidence,
}

impl RunDatasourceBinding {
    pub fn from_validated(
        run_id: crate::RunId,
        scope: DatasourceScope,
        selection_version: u64,
        revision: &DatasourceRevision,
        evidence: &ValidationEvidence,
        current: &DatasourceValidationInputs,
    ) -> CoreResult<Self> {
        let actual = DatasourceValidationInputs::new(
            revision,
            &current.capability,
            current.policy_digest.clone(),
        )?;
        if scope.workspace_id != revision.identity().workspace_id
            || !evidence.matches(&actual)
            || &actual != current
            || selection_version == 0
        {
            return Err(CoreError::validation(
                "datasource_validation_stale",
                "binding requires matching current evidence and selection",
            ));
        }
        Ok(Self {
            schema_version: 1,
            run_id,
            scope,
            selection_version,
            evidence: evidence.clone(),
        })
    }

    pub fn validate_supported(&self) -> CoreResult<()> {
        if self.schema_version != 1 {
            return Err(CoreError::UnsupportedSchemaVersion {
                version: self.schema_version,
            });
        }
        if self.selection_version == 0
            || self.scope.workspace_id != self.revision().workspace_id
            || !self.evidence.matches(&self.evidence.inputs)
        {
            return Err(CoreError::validation(
                "invalid_datasource_binding",
                "datasource binding is invalid",
            ));
        }
        Ok(())
    }

    pub fn run_id(&self) -> crate::RunId {
        self.run_id
    }
    pub fn scope(&self) -> DatasourceScope {
        self.scope
    }
    pub fn revision(&self) -> DatasourceRevisionId {
        self.evidence.inputs.revision
    }
    pub fn source_id(&self) -> &SourceId {
        &self.evidence.inputs.capability.source_id
    }
    pub fn evidence(&self) -> &ValidationEvidence {
        &self.evidence
    }
    pub fn selection_version(&self) -> u64 {
        self.selection_version
    }
    pub fn credential(&self) -> Option<DatasourceSecretRef> {
        self.evidence.inputs.credential
    }
    pub fn adapter_id(&self) -> &AdapterId {
        &self.evidence.inputs.adapter_id
    }
    pub fn adapter_version(&self) -> &AdapterVersion {
        &self.evidence.inputs.adapter_version
    }
    pub fn digest(&self) -> CoreResult<DatasourceDigest> {
        DatasourceDigest::of(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunDatasourceContext {
    pub schema_version: u32,
    pub binding: RunDatasourceBinding,
    pub data_scope: crate::AllowedDataScope,
    pub result_policy: BTreeMap<String, BTreeMap<String, crate::ColumnPolicy>>,
    pub query_budget: crate::QueryBudget,
    pub tools: Vec<String>,
    pub context_namespace: DatasourceDigest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DsErrorCode {
    InvalidField,
    DuplicateName,
    ConfigIncompatible,
    CredentialMissing,
    CredentialExpired,
    ProtectionUnavailable,
    AuthenticationFailed,
    TargetMissing,
    FileUnreadable,
    PermissionDenied,
    ReadOnlyUnproven,
    Timeout,
    Network,
    Protocol,
    CapabilityMissing,
    PolicyDenied,
    ValidationStale,
    Conflict,
    InUse,
    Storage,
    Cancelled,
    UnsupportedType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DsRemediation {
    EditConfiguration,
    ReplaceCredential,
    RepairProtection,
    CheckConnectivity,
    RepairPolicy,
    Revalidate,
    Refresh,
    WaitOrCancelRun,
    Retry,
    UpgradeAdapter,
}

/// Driver messages and source errors intentionally cannot enter this public error envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("datasource operation failed: {code:?}; action: {remediation:?}")]
pub struct DsError {
    pub code: DsErrorCode,
    pub field: Option<FieldId>,
    pub remediation: DsRemediation,
    pub operation_id: Option<crate::OperationId>,
}

pub type DsResult<T> = Result<T, DsError>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevisionState {
    Draft,
    Ready,
    Invalid(DsErrorCode),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasourceProfile {
    pub schema_version: u32,
    pub workspace_id: WorkspaceId,
    pub profile_id: ProfileId,
    pub source_id: Option<SourceId>,
    pub name: DatasourceName,
    pub head_revision: NonZeroU64,
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasourceDetail {
    pub schema_version: u32,
    pub profile: DatasourceProfile,
    pub revision: DatasourceRevision,
    pub state: RevisionState,
    pub validation: Option<ValidationEvidence>,
}

impl DatasourceDetail {
    /// A Ready label never overrides mismatched or absent evidence.
    pub fn is_ready(&self, inputs: &DatasourceValidationInputs) -> bool {
        let actual = DatasourceValidationInputs::new(
            &self.revision,
            inputs.capability(),
            inputs.policy_digest().clone(),
        );
        self.schema_version == 1
            && self.profile.schema_version == 1
            && self.profile.deleted_at.is_none()
            && self.state == RevisionState::Ready
            && self.revision.identity() == inputs.revision()
            && self.profile.workspace_id == inputs.revision().workspace_id
            && self.profile.profile_id == inputs.revision().profile_id
            && self.profile.source_id.as_ref() == Some(&inputs.capability().source_id)
            && actual.as_ref().is_ok_and(|actual| actual == inputs)
            && self.validation.as_ref().is_some_and(|v| v.matches(inputs))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionSnapshot {
    pub schema_version: u32,
    pub scope: DatasourceScope,
    /// None is a committed unconfigured choice, not a request to inherit the default.
    pub current: Option<DatasourceRevisionId>,
    pub workspace_default: Option<DatasourceRevisionId>,
    pub selection_version: u64,
    pub default_version: u64,
    pub header: Option<DatasourceHeader>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasourceHeader {
    pub name: DatasourceName,
    pub adapter_id: AdapterId,
    pub revision: DatasourceRevisionId,
    pub context_digest: DatasourceDigest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasourceSnapshot {
    pub schema_version: u32,
    /// Workspace-wide management CAS version, independent of each selection's version.
    pub version: u64,
    pub profiles: Vec<DatasourceDetail>,
    pub selection: SelectionSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasourceView {
    pub schema_version: u32,
    pub catalog: Vec<ConnectorDescriptor>,
    pub snapshot: DatasourceSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasourceWriteContext {
    pub command_id: crate::CommandId,
    pub scope: DatasourceScope,
    pub expected_version: u64,
    pub expected_head_revision: Option<NonZeroU64>,
}

pub struct SaveDatasource {
    pub write: DatasourceWriteContext,
    pub profile_id: Option<ProfileId>,
    pub name: DatasourceName,
    pub adapter_id: AdapterId,
    pub adapter_version: AdapterVersion,
    pub config_version: u32,
    pub fields: BTreeMap<FieldId, FieldValue>,
    pub context: DatabaseContext,
    pub secret: SecretEdit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationMode {
    Local,
    Connection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidateDatasource {
    pub write: DatasourceWriteContext,
    pub revision: DatasourceRevisionId,
    pub mode: ValidationMode,
    pub operation_id: crate::OperationId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatasourceSelectionKind {
    Session,
    WorkspaceDefault,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectDatasource {
    pub write: DatasourceWriteContext,
    pub revision: DatasourceRevisionId,
    pub kind: DatasourceSelectionKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeleteDatasourceDisposition {
    Replacement(DatasourceRevisionId),
    ConfirmUnconfigured,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteDatasource {
    pub write: DatasourceWriteContext,
    pub profile_id: ProfileId,
    pub disposition: DeleteDatasourceDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationReport {
    pub schema_version: u32,
    pub revision: DatasourceRevisionId,
    pub mode: ValidationMode,
    pub fields: Vec<FieldIssue>,
    pub evidence: Option<ValidationEvidence>,
    pub state: RevisionState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasourceDoctorRequest {
    pub scope: DatasourceScope,
    pub revision: Option<DatasourceRevisionId>,
    pub operation_id: crate::OperationId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasourceDoctorReport {
    pub schema_version: u32,
    pub validation: Option<ValidationReport>,
    pub findings: Vec<DsError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorSupport {
    Registered,
    Supported,
    Incompatible,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorDescriptor {
    pub schema_version: u32,
    pub adapter_id: AdapterId,
    pub adapter_version: AdapterVersion,
    pub config_version: u32,
    pub contract_version: u32,
    pub display_name: String,
    pub support: ConnectorSupport,
    pub fields: Vec<DatasourceField>,
    pub capability: crate::CapabilityDescriptor,
    pub max_connections: NonZeroU64,
    pub release_evidence: Option<DatasourceDigest>,
}

/// Scoped credentials have neither serialization nor Debug. Dropping the lease zeroizes bytes.
pub struct SecretLease {
    pub reference: DatasourceSecretRef,
    pub value: SecretValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtectionStatus {
    OwnerOnlyEncryptedFile,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretMutationPhase {
    Prepared,
    VaultWritten,
    Committed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretMutation {
    pub schema_version: u32,
    pub mutation_id: crate::OperationId,
    pub write: DatasourceWriteContext,
    pub profile_id: ProfileId,
    pub old: Option<DatasourceSecretRef>,
    pub new: Option<DatasourceSecretRef>,
    pub phase: SecretMutationPhase,
    pub command_digest: DatasourceDigest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum DatasourceChange {
    SaveRevision {
        profile: DatasourceProfile,
        revision: DatasourceRevision,
        mutation_id: Option<crate::OperationId>,
    },
    Validation {
        revision: DatasourceRevisionId,
        state: RevisionState,
        evidence: Option<ValidationEvidence>,
    },
    Selection {
        revision: DatasourceRevisionId,
        kind: DatasourceSelectionKind,
    },
    Delete {
        profile_id: ProfileId,
        disposition: DeleteDatasourceDisposition,
    },
    SecretJournal {
        mutation: SecretMutation,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasourceCommit {
    pub schema_version: u32,
    pub write: DatasourceWriteContext,
    pub command_digest: DatasourceDigest,
    pub change: DatasourceChange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasourceReceipt {
    pub schema_version: u32,
    pub command_id: crate::CommandId,
    pub command_digest: DatasourceDigest,
    pub committed_version: u64,
    pub snapshot: DatasourceSnapshot,
}

/// This input is assembled only after matching the user's existing Source Policy to the exact
/// target. It is not an authorization grant made by the Profile or Connector catalog.
#[derive(Debug, Clone)]
pub struct DatasourceGovernanceContext {
    pub data_scope: crate::AllowedDataScope,
    pub result_policy: BTreeMap<String, BTreeMap<String, crate::ColumnPolicy>>,
    pub budget: crate::QueryBudget,
    pub policy_digest: DatasourceDigest,
    pub allowed_roots: Vec<std::path::PathBuf>,
}

pub struct ConnectorOpenInput {
    pub revision: DatasourceRevision,
    pub secret: Option<SecretLease>,
    pub governance: DatasourceGovernanceContext,
}

pub struct ResolvedRunDatasource {
    pub context: RunDatasourceContext,
    pub connector: Arc<dyn ManagedConnector>,
}

#[async_trait]
pub trait DatasourceManagementApi: Send + Sync {
    async fn view(&self, scope: DatasourceScope) -> DsResult<DatasourceView>;
    async fn save(&self, request: SaveDatasource) -> DsResult<DatasourceDetail>;
    async fn validate(&self, request: ValidateDatasource) -> DsResult<ValidationReport>;
    async fn select(&self, request: SelectDatasource) -> DsResult<SelectionSnapshot>;
    async fn delete(&self, request: DeleteDatasource) -> DsResult<SelectionSnapshot>;
    async fn doctor(&self, request: DatasourceDoctorRequest) -> DsResult<DatasourceDoctorReport>;
    async fn cancel(&self, operation: crate::OperationId) -> DsResult<()>;
    async fn receipt(&self, command: crate::CommandId) -> DsResult<Option<DatasourceReceipt>>;
}

#[async_trait]
pub trait DatasourceRepository: Send + Sync {
    async fn load(&self, scope: DatasourceScope) -> DsResult<DatasourceSnapshot>;
    async fn load_revision(&self, revision: DatasourceRevisionId) -> DsResult<DatasourceDetail>;
    async fn commit(&self, change: DatasourceCommit) -> DsResult<DatasourceReceipt>;
    async fn receipt(&self, command: crate::CommandId) -> DsResult<Option<DatasourceReceipt>>;
    async fn pending_secret_mutations(
        &self,
        workspace: WorkspaceId,
    ) -> DsResult<Vec<SecretMutation>>;
    async fn load_run_binding(&self, run: crate::RunId) -> DsResult<RunDatasourceBinding>;
    /// Retire an uncommitted/obsolete generation atomically after checking durable Run leases.
    /// The caller may remove the vault file only after this succeeds. Repeated claims are safe.
    async fn claim_secret_cleanup(&self, reference: DatasourceSecretRef) -> DsResult<()>;
    async fn finish_secret_cleanup(&self, reference: DatasourceSecretRef) -> DsResult<()>;
    async fn obsolete_secret_generations(
        &self,
        workspace: WorkspaceId,
    ) -> DsResult<Vec<DatasourceSecretRef>>;
    /// Acknowledge successful file cleanup; failed cleanup must retain the journal for retry.
    async fn finish_secret_mutation(&self, mutation: crate::OperationId) -> DsResult<()>;
}

#[async_trait]
pub trait DatasourceVault: Send + Sync {
    async fn protection(&self) -> DsResult<ProtectionStatus>;
    async fn write(&self, reference: DatasourceSecretRef, value: SecretValue) -> DsResult<()>;
    async fn read(&self, reference: DatasourceSecretRef) -> DsResult<SecretLease>;
    async fn remove(&self, reference: DatasourceSecretRef) -> DsResult<()>;
}

pub trait ConnectorCatalog: Send + Sync {
    fn descriptors(&self) -> DsResult<Vec<ConnectorDescriptor>>;
    fn factory(
        &self,
        id: &AdapterId,
        version: &AdapterVersion,
    ) -> DsResult<Arc<dyn ConnectorFactory>>;
}

#[async_trait]
pub trait ConnectorFactory: Send + Sync {
    fn validate_config(&self, input: &DatasourceRevision) -> Vec<FieldIssue>;
    async fn open(&self, input: ConnectorOpenInput) -> DsResult<Arc<dyn ManagedConnector>>;
}

#[async_trait]
pub trait ManagedConnector:
    crate::CatalogReader
    + crate::QueryPreflightReader
    + crate::SqlQueryExecutor
    + crate::FreshnessReader
    + Send
    + Sync
{
    async fn probe(&self) -> DsResult<ProbeEvidence>;
    async fn close(&self) -> DsResult<()>;
}

#[async_trait]
pub trait RunDatasourceResolver: Send + Sync {
    async fn resolve(&self, run_id: crate::RunId) -> DsResult<ResolvedRunDatasource>;
    async fn release(&self, run_id: crate::RunId) -> DsResult<()>;
    async fn close(&self) -> DsResult<()>;
}

#[async_trait]
pub trait RunDatasourceBindingSource: Send + Sync {
    /// A new Run needs an explicit Session scope. A retry may recover that scope only from its
    /// durable prior Run binding; otherwise production fails closed rather than guessing a
    /// Session or falling back to the Workspace default.
    async fn bind_new_run(
        &self,
        run_id: crate::RunId,
        scope: Option<DatasourceScope>,
        retry_of: Option<crate::RunId>,
    ) -> DsResult<RunDatasourceBinding>;
}

/// Display name with an ASCII-folded, Workspace-local uniqueness key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DatasourceName(String);

impl DatasourceName {
    pub fn new(value: impl Into<String>) -> CoreResult<Self> {
        let value = value.into();
        let value = value.trim();
        if value.is_empty() || value.chars().any(char::is_control) {
            return Err(CoreError::validation(
                "invalid_datasource_name",
                "datasource name must be nonempty and contain no control characters",
            ));
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn uniqueness_key(&self) -> String {
        self.0.to_ascii_lowercase()
    }
}

impl TryFrom<String> for DatasourceName {
    type Error = CoreError;

    fn try_from(value: String) -> CoreResult<Self> {
        Self::new(value)
    }
}

impl From<DatasourceName> for String {
    fn from(value: DatasourceName) -> Self {
        value.0
    }
}

/// Immutable, non-sensitive locator in the Datasource Vault namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasourceSecretRef {
    workspace_id: WorkspaceId,
    profile_id: ProfileId,
    generation: NonZeroU64,
}

impl DatasourceSecretRef {
    pub fn new(
        workspace_id: WorkspaceId,
        profile_id: ProfileId,
        generation: u64,
    ) -> CoreResult<Self> {
        let generation = NonZeroU64::new(generation).ok_or_else(|| {
            CoreError::validation("invalid_datasource_generation", "generation starts at one")
        })?;
        Ok(Self {
            workspace_id,
            profile_id,
            generation,
        })
    }

    pub fn workspace_id(self) -> WorkspaceId {
        self.workspace_id
    }

    pub fn profile_id(self) -> ProfileId {
        self.profile_id
    }

    pub fn generation(self) -> u64 {
        self.generation.get()
    }
}

/// A masked UI value is never a replacement credential. Only explicit replacement carries bytes.
///
/// ```compile_fail
/// let edit = ys_agent_core::SecretEdit::Keep;
/// let _ = serde_json::to_string(&edit);
/// ```
///
/// ```compile_fail
/// let edit = ys_agent_core::SecretEdit::Keep;
/// let _ = format!("{edit:?}");
/// ```
pub enum SecretEdit {
    Keep,
    Replace(SecretValue),
    Remove,
}

impl SecretEdit {
    /// Replacement is assigned a fresh generation by the journal transaction.
    pub fn retained_reference(
        &self,
        current: Option<DatasourceSecretRef>,
    ) -> Option<DatasourceSecretRef> {
        match self {
            Self::Keep => current,
            Self::Replace(_) | Self::Remove => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct FieldId(String);

impl FieldId {
    pub fn new(value: impl Into<String>) -> CoreResult<Self> {
        let value = value.into();
        if value.is_empty()
            || !value
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            return Err(CoreError::validation(
                "invalid_field_id",
                "invalid datasource field identifier",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for FieldId {
    type Error = CoreError;

    fn try_from(value: String) -> CoreResult<Self> {
        Self::new(value)
    }
}

impl From<FieldId> for String {
    fn from(value: FieldId) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FieldInput {
    Text,
    Integer { min: i64, max: i64 },
    Boolean,
    Choice { choices: Vec<String> },
    ExistingFile,
    Secret,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum FieldValue {
    Text(String),
    Integer(i64),
    Boolean(bool),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasourceField {
    pub id: FieldId,
    pub label: String,
    pub required: bool,
    pub input: FieldInput,
    pub default: Option<FieldValue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldIssueCode {
    Missing,
    Invalid,
    Unknown,
    SecretInOrdinaryConfig,
    InvalidDescriptor,
}

/// Contains identifiers and stable categories only, never the rejected value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldIssue {
    pub field: FieldId,
    pub code: FieldIssueCode,
}

/// Pure field validation; file existence and all connection I/O belong to the driver.
pub fn validate_datasource_fields(
    descriptors: &[DatasourceField],
    fields: &BTreeMap<FieldId, FieldValue>,
    has_secret: bool,
    require_complete: bool,
) -> Vec<FieldIssue> {
    let mut issues = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for descriptor in descriptors {
        let secret = matches!(descriptor.input, FieldInput::Secret);
        let invalid_descriptor = !seen.insert(&descriptor.id)
            || matches!(descriptor.input, FieldInput::Integer { min, max } if min > max)
            || matches!(&descriptor.input, FieldInput::Choice { choices } if choices.is_empty())
            || descriptor
                .default
                .as_ref()
                .is_some_and(|v| secret || !field_value_matches(&descriptor.input, v));
        if invalid_descriptor {
            issues.push(FieldIssue {
                field: descriptor.id.clone(),
                code: FieldIssueCode::InvalidDescriptor,
            });
            continue;
        }
        let issue = match fields.get(&descriptor.id) {
            Some(_) if secret => Some(FieldIssueCode::SecretInOrdinaryConfig),
            Some(value) if !field_value_matches(&descriptor.input, value) => {
                Some(FieldIssueCode::Invalid)
            }
            None if require_complete
                && descriptor.required
                && if secret {
                    !has_secret
                } else {
                    descriptor.default.is_none()
                } =>
            {
                Some(FieldIssueCode::Missing)
            }
            _ => None,
        };
        if let Some(code) = issue {
            issues.push(FieldIssue {
                field: descriptor.id.clone(),
                code,
            });
        }
    }
    for field in fields.keys().filter(|field| !seen.contains(field)) {
        issues.push(FieldIssue {
            field: field.clone(),
            code: FieldIssueCode::Unknown,
        });
    }
    issues
}

fn field_value_matches(input: &FieldInput, value: &FieldValue) -> bool {
    match (input, value) {
        (FieldInput::Text | FieldInput::ExistingFile, FieldValue::Text(value)) => {
            !value.trim().is_empty()
                && !value.chars().any(char::is_control)
                && !value.contains("://")
        }
        (FieldInput::Integer { min, max }, FieldValue::Integer(value)) => {
            (min..=max).contains(&value)
        }
        (FieldInput::Boolean, FieldValue::Boolean(_)) => true,
        (FieldInput::Choice { choices }, FieldValue::Text(value)) => choices.contains(value),
        _ => false,
    }
}
