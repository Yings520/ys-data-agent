use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};
use ys_agent_core::{
    AdapterId, AllowedDataScope, ColumnPolicy, DatabaseContext, DatasourceDigest,
    DatasourceGovernanceContext, DatasourceRevision, DsError, DsErrorCode, DsRemediation, DsResult,
    FieldId, FieldValue, QueryBudget, SourceId,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyDocument {
    schema_version: u32,
    allowed_sources: BTreeMap<String, SourceRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceRule {
    relations: BTreeMap<String, RelationRule>,
    target: SourceTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RelationRule {
    columns: BTreeMap<String, ColumnPolicy>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum SourceTarget {
    File {
        adapter_id: AdapterId,
        canonical_path: PathBuf,
        allowed_roots: Vec<PathBuf>,
    },
    Database {
        adapter_id: AdapterId,
        host: String,
        port: u16,
        database: String,
        schema: String,
    },
}

#[derive(Clone)]
pub struct SourcePolicy {
    document: PolicyDocument,
    budget: QueryBudget,
    digest: DatasourceDigest,
}

impl std::fmt::Debug for SourcePolicy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SourcePolicy")
            .field("schema_version", &self.document.schema_version)
            .field("source_count", &self.document.allowed_sources.len())
            .finish_non_exhaustive()
    }
}

impl SourcePolicy {
    /// Keeps management available before a v2 policy is supplied while denying every physical
    /// target. It cannot produce validation evidence or authorize a Run.
    pub fn deny_all(budget: QueryBudget) -> Self {
        let document = PolicyDocument {
            schema_version: 2,
            allowed_sources: BTreeMap::new(),
        };
        let digest = DatasourceDigest::of(&(&document, &budget))
            .expect("the empty policy and validated budget serialize");
        Self {
            document,
            budget,
            digest,
        }
    }

    pub fn from_json_bytes(bytes: &[u8], budget: QueryBudget) -> DsResult<Self> {
        let document: PolicyDocument =
            serde_json::from_slice(bytes).map_err(|_| error(DsErrorCode::ConfigIncompatible))?;
        if document.schema_version != 2
            || document.allowed_sources.is_empty()
            || budget.max_sql_bytes == 0
            || budget.statement_timeout_ms == 0
            || budget.acquire_timeout_ms == 0
            || budget.max_rows == 0
            || budget.max_result_bytes == 0
            || budget.max_concurrency == 0
        {
            return Err(error(DsErrorCode::ConfigIncompatible));
        }
        for (source, rule) in &document.allowed_sources {
            if source.trim().is_empty() || rule.relations.is_empty() {
                return Err(error(DsErrorCode::ConfigIncompatible));
            }
            if rule
                .relations
                .values()
                .any(|relation| relation.columns.is_empty())
                || !valid_target(&rule.target)
            {
                return Err(error(DsErrorCode::ConfigIncompatible));
            }
        }
        let digest = DatasourceDigest::of(&(&document, &budget))
            .map_err(|_| error(DsErrorCode::ConfigIncompatible))?;
        Ok(Self {
            document,
            budget,
            digest,
        })
    }

    pub fn match_target(
        &self,
        adapter: &AdapterId,
        fields: &BTreeMap<FieldId, FieldValue>,
        context: &DatabaseContext,
    ) -> DsResult<(SourceId, DatasourceGovernanceContext)> {
        let matches = self
            .document
            .allowed_sources
            .iter()
            .filter(|(_, rule)| target_matches(&rule.target, adapter, fields, context))
            .collect::<Vec<_>>();
        let [(name, rule)] = matches.as_slice() else {
            return Err(error(DsErrorCode::PolicyDenied));
        };
        let source = SourceId::new((*name).clone());
        let relations = rule
            .relations
            .iter()
            .map(|(name, relation)| (name.clone(), relation.columns.clone()))
            .collect::<BTreeMap<_, _>>();
        let allowed_roots = match &rule.target {
            SourceTarget::File { allowed_roots, .. } => allowed_roots.clone(),
            SourceTarget::Database { .. } => Vec::new(),
        };
        Ok((
            source.clone(),
            DatasourceGovernanceContext {
                data_scope: AllowedDataScope {
                    workspace_id: ys_agent_core::WorkspaceId::new(),
                    source_id: source.as_str().to_owned(),
                    relations: relations.clone(),
                },
                result_policy: relations,
                budget: self.budget.clone(),
                policy_digest: self.digest.clone(),
                allowed_roots,
            },
        ))
    }

    pub fn governance_for(
        &self,
        revision: &DatasourceRevision,
    ) -> DsResult<DatasourceGovernanceContext> {
        let input = revision.input();
        let (source, mut governance) =
            self.match_target(&input.adapter_id, &input.fields, &input.context)?;
        if input.source_id.as_ref() != Some(&source) {
            return Err(error(DsErrorCode::PolicyDenied));
        }
        governance.data_scope.workspace_id = input.workspace_id;
        Ok(governance)
    }
}

fn target_matches(
    target: &SourceTarget,
    adapter: &AdapterId,
    fields: &BTreeMap<FieldId, FieldValue>,
    context: &DatabaseContext,
) -> bool {
    match (target, context) {
        (
            SourceTarget::File {
                adapter_id,
                canonical_path,
                ..
            },
            DatabaseContext::File {
                canonical_path: actual,
            },
        ) => {
            adapter_id == adapter
                && canonical_path == actual
                && fields.get(&field("database_path"))
                    == Some(&FieldValue::Text(actual.to_string_lossy().into_owned()))
        }
        (
            SourceTarget::Database {
                adapter_id,
                host,
                port,
                database,
                schema,
            },
            DatabaseContext::Database {
                catalog,
                database: actual_database,
                schema: actual_schema,
            },
        ) => {
            adapter_id == adapter
                && actual_database == database
                && actual_schema == schema
                && catalog.as_deref() == Some(format!("{host}:{port}").as_str())
                && fields.get(&field("host")) == Some(&FieldValue::Text(host.clone()))
                && fields.get(&field("port")) == Some(&FieldValue::Integer(i64::from(*port)))
                && fields.get(&field("database")) == Some(&FieldValue::Text(database.clone()))
                && fields.get(&field("schema")) == Some(&FieldValue::Text(schema.clone()))
        }
        _ => false,
    }
}

fn valid_target(target: &SourceTarget) -> bool {
    match target {
        SourceTarget::File {
            canonical_path,
            allowed_roots,
            ..
        } => {
            canonical_path.is_absolute()
                && safe_path(canonical_path)
                && !allowed_roots.is_empty()
                && allowed_roots
                    .iter()
                    .all(|root| root.is_absolute() && safe_path(root))
                && allowed_roots
                    .iter()
                    .any(|root| canonical_path.starts_with(root) && canonical_path != root)
        }
        SourceTarget::Database {
            host,
            port,
            database,
            schema,
            ..
        } => {
            !host.is_empty()
                && *port > 0
                && !database.is_empty()
                && !schema.is_empty()
                && [host, database, schema]
                    .into_iter()
                    .all(|value| !value.chars().any(char::is_control) && !value.contains("://"))
        }
    }
}

fn safe_path(path: &std::path::Path) -> bool {
    path.to_str().is_some()
        && path.components().all(|component| {
            !matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
}

fn field(name: &str) -> FieldId {
    FieldId::new(name).expect("static field")
}

fn error(code: DsErrorCode) -> DsError {
    DsError {
        code,
        field: None,
        remediation: match code {
            DsErrorCode::PolicyDenied => DsRemediation::RepairPolicy,
            _ => DsRemediation::EditConfiguration,
        },
        operation_id: None,
    }
}
