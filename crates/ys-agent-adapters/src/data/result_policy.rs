use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::Deserialize;
use ys_agent_core::{
    AllowedDataScope, CellValue, ColumnPolicy, CoreError, CoreResult, PrincipalId, QueryResult,
    Sensitivity, SourceId, WorkspaceId,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnAction {
    Allow,
    Redact,
    LocalArtifactOnly,
    Deny,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyFile {
    schema_version: u32,
    allowed_sources: BTreeMap<String, SourceRule>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceRule {
    relations: BTreeMap<String, RelationRule>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RelationRule {
    columns: BTreeMap<String, ColumnAction>,
}

#[derive(Debug, Clone)]
pub struct ResultPolicy {
    sources: BTreeMap<String, SourceRule>,
}

impl ResultPolicy {
    /// Consumes an already-authorized immutable scope; this does not grant target authority.
    pub(crate) fn from_scope(scope: &AllowedDataScope) -> Self {
        Self {
            sources: [(
                scope.source_id.clone(),
                SourceRule {
                    relations: scope
                        .relations
                        .iter()
                        .map(|(name, columns)| {
                            (
                                name.clone(),
                                RelationRule {
                                    columns: columns
                                        .iter()
                                        .map(|(column, policy)| {
                                            (
                                                column.clone(),
                                                match policy {
                                                    ColumnPolicy::Allow => ColumnAction::Allow,
                                                    ColumnPolicy::Redact => ColumnAction::Redact,
                                                    ColumnPolicy::LocalArtifactOnly => {
                                                        ColumnAction::LocalArtifactOnly
                                                    }
                                                    ColumnPolicy::Deny => ColumnAction::Deny,
                                                },
                                            )
                                        })
                                        .collect(),
                                },
                            )
                        })
                        .collect(),
                },
            )]
            .into(),
        }
    }

    pub fn from_json_bytes(bytes: &[u8]) -> CoreResult<Self> {
        let parsed: PolicyFile = serde_json::from_slice(bytes).map_err(|error| {
            CoreError::validation("invalid_query_policy", safe_message("parse policy", error))
        })?;

        if parsed.schema_version != 1 {
            return Err(CoreError::validation(
                "unsupported_query_policy_version",
                format!(
                    "expected schema_version 1, received {}",
                    parsed.schema_version
                ),
            ));
        }
        if parsed.allowed_sources.is_empty() {
            return Err(CoreError::validation(
                "missing_source_scope",
                "query policy needs at least one source",
            ));
        }

        for (source, source_rule) in &parsed.allowed_sources {
            validate_name(source, "source")?;
            if source_rule.relations.is_empty() {
                return Err(CoreError::validation(
                    "missing_relation_scope",
                    format!("source {source} has no relation scope"),
                ));
            }
            for (relation, relation_rule) in &source_rule.relations {
                validate_qualified_identifier(relation, "relation")?;
                if relation_rule.columns.is_empty() {
                    return Err(CoreError::validation(
                        "missing_column_scope",
                        format!("relation {relation} has no column scope"),
                    ));
                }
                for column in relation_rule.columns.keys() {
                    validate_name(column, "column")?;
                }
            }
        }

        Ok(Self {
            sources: parsed.allowed_sources,
        })
    }

    pub fn allowed_scope(
        &self,
        workspace_id: WorkspaceId,
        source_id: &SourceId,
    ) -> CoreResult<AllowedDataScope> {
        let source = self.sources.get(source_id.as_str()).ok_or_else(|| {
            CoreError::validation(
                "source_not_allowed",
                format!("source {} is not in query policy", source_id.as_str()),
            )
        })?;

        let relations = source
            .relations
            .iter()
            .map(|(relation, rule)| {
                let columns = rule
                    .columns
                    .iter()
                    .map(|(column, action)| (column.clone(), (*action).into()))
                    .collect();
                (relation.clone(), columns)
            })
            .collect();

        Ok(AllowedDataScope {
            workspace_id,
            source_id: source_id.as_str().to_owned(),
            relations,
        })
    }

    pub fn action(
        &self,
        source_id: &SourceId,
        relations: &[String],
        column: &str,
    ) -> Option<ColumnAction> {
        let source = self.sources.get(source_id.as_str())?;
        relations
            .iter()
            .filter_map(|relation| {
                find_relation(&source.relations, relation)
                    .and_then(|rule| rule.columns.get(&column.to_ascii_lowercase()).copied())
            })
            .max_by_key(|action| action_severity(*action))
    }

    pub fn column_sensitivity(
        &self,
        source_id: &SourceId,
        relation: &str,
        column: &str,
    ) -> Sensitivity {
        match self.action(source_id, &[relation.to_owned()], column) {
            Some(ColumnAction::Allow) => Sensitivity::Internal,
            Some(ColumnAction::Redact | ColumnAction::LocalArtifactOnly | ColumnAction::Deny) => {
                Sensitivity::Restricted
            }
            None => Sensitivity::Restricted,
        }
    }
}

impl From<ColumnAction> for ColumnPolicy {
    fn from(value: ColumnAction) -> Self {
        match value {
            ColumnAction::Allow => Self::Allow,
            ColumnAction::Redact => Self::Redact,
            ColumnAction::LocalArtifactOnly => Self::LocalArtifactOnly,
            ColumnAction::Deny => Self::Deny,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DecodedQueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<CellValue>>,
    pub truncated: bool,
    pub remote_query_id: Option<String>,
    pub warning_codes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RestrictedResultContext {
    pub owner: PrincipalId,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct RestrictedResultPayload {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<CellValue>>,
    pub owner: PrincipalId,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct GovernedQueryResult {
    pub model_result: QueryResult,
    pub restricted_payload: Option<RestrictedResultPayload>,
}

impl ResultPolicy {
    pub(crate) fn apply(
        &self,
        source_id: &SourceId,
        referenced_relations: &[String],
        referenced_columns: &[String],
        decoded: DecodedQueryResult,
        max_result_bytes: usize,
        restricted_context: Option<&RestrictedResultContext>,
    ) -> CoreResult<GovernedQueryResult> {
        let mut output_names = BTreeSet::new();
        if decoded
            .columns
            .iter()
            .any(|column| !output_names.insert(column.to_ascii_lowercase()))
        {
            return Err(CoreError::validation(
                "ambiguous_result_columns",
                "duplicate output names cannot preserve column policy provenance",
            ));
        }
        let fallback_action = referenced_columns
            .iter()
            .filter_map(|column| self.action(source_id, referenced_relations, column))
            .max_by_key(|action| action_severity(*action));

        let mut warnings = decoded
            .warning_codes
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut restricted_rows = Vec::with_capacity(decoded.rows.len());
        let mut has_local_only = false;
        let mut model_rows = Vec::with_capacity(decoded.rows.len());

        for row in &decoded.rows {
            let mut model_row = Vec::with_capacity(row.len());
            let mut restricted_row = Vec::with_capacity(row.len());

            for (index, value) in row.iter().enumerate() {
                let output_name = decoded.columns.get(index).ok_or_else(|| {
                    CoreError::validation("result_shape_mismatch", "row is wider than columns")
                })?;
                let direct = self.action(source_id, referenced_relations, output_name);
                let output_is_input = referenced_columns
                    .iter()
                    .any(|column| column.eq_ignore_ascii_case(output_name));
                let action = if output_is_input {
                    direct
                } else {
                    direct
                        .into_iter()
                        .chain(fallback_action)
                        .max_by_key(|action| action_severity(*action))
                }
                .unwrap_or(ColumnAction::Allow);

                match action {
                    ColumnAction::Allow => {
                        model_row.push(value.clone());
                        restricted_row.push(value.clone());
                    }
                    ColumnAction::Redact => {
                        model_row.push(CellValue::Text("[REDACTED]".to_owned()));
                        restricted_row.push(value.clone());
                        warnings.insert("restricted_column_redacted".to_owned());
                    }
                    ColumnAction::LocalArtifactOnly => {
                        model_row.push(CellValue::Text("[LOCAL_ARTIFACT_ONLY]".to_owned()));
                        restricted_row.push(value.clone());
                        has_local_only = true;
                        warnings.insert("restricted_column_local_artifact_only".to_owned());
                    }
                    ColumnAction::Deny => {
                        return Err(CoreError::validation(
                            "column_denied",
                            format!("result column {output_name} is denied"),
                        ));
                    }
                }
            }

            model_rows.push(model_row);
            restricted_rows.push(restricted_row);
        }

        let restricted_payload = if has_local_only {
            let context = restricted_context.ok_or_else(|| {
                CoreError::validation(
                    "missing_restricted_artifact_context",
                    "local_artifact_only data needs an owner and expiry",
                )
            })?;
            Some(RestrictedResultPayload {
                columns: decoded.columns.clone(),
                rows: restricted_rows,
                owner: context.owner,
                expires_at: context.expires_at,
            })
        } else {
            None
        };

        let preview_value = serde_json::json!({
            "columns": &decoded.columns,
            "rows": &model_rows,
            "truncated": decoded.truncated,
        });
        let model_preview = serde_json::to_string(&preview_value).map_err(|error| {
            CoreError::validation(
                "result_serialization_failed",
                safe_message("preview", error),
            )
        })?;
        if model_preview.len() > max_result_bytes {
            return Err(CoreError::validation(
                "result_byte_budget_exceeded",
                format!(
                    "governed preview is {} bytes; maximum is {max_result_bytes}",
                    model_preview.len()
                ),
            ));
        }

        Ok(GovernedQueryResult {
            model_result: QueryResult {
                columns: decoded.columns,
                row_count: model_rows.len(),
                rows: model_rows,
                truncated: decoded.truncated,
                remote_query_id: decoded.remote_query_id,
                serialized_bytes: model_preview.len(),
                warning_codes: warnings.into_iter().collect(),
                model_preview,
            },
            restricted_payload,
        })
    }
}

fn action_severity(action: ColumnAction) -> u8 {
    match action {
        ColumnAction::Allow => 0,
        ColumnAction::Redact => 1,
        ColumnAction::LocalArtifactOnly => 2,
        ColumnAction::Deny => 3,
    }
}

fn find_relation<'a>(
    relations: &'a BTreeMap<String, RelationRule>,
    requested: &str,
) -> Option<&'a RelationRule> {
    let requested = requested.to_ascii_lowercase();
    if let Some(rule) = relations.get(&requested) {
        return Some(rule);
    }

    let mut matches = relations.iter().filter_map(|(name, rule)| {
        name.rsplit_once('.')
            .filter(|(_, suffix)| *suffix == requested)
            .map(|_| rule)
    });
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}

fn validate_qualified_identifier(value: &str, kind: &'static str) -> CoreResult<()> {
    if value == "*"
        || value
            .split('.')
            .any(|part| validate_name(part, kind).is_err())
    {
        return Err(CoreError::validation(
            "unsafe_policy_identifier",
            format!("{kind} {value:?} is not a safe exact identifier"),
        ));
    }
    Ok(())
}

fn validate_name(value: &str, kind: &'static str) -> CoreResult<()> {
    let mut chars = value.chars();
    let first_is_safe = chars
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic());
    let rest_is_safe = chars.all(|character| character == '_' || character.is_ascii_alphanumeric());
    if value == "*" || !first_is_safe || !rest_is_safe {
        return Err(CoreError::validation(
            "unsafe_policy_identifier",
            format!("{kind} {value:?} is not a safe exact identifier"),
        ));
    }
    Ok(())
}

fn safe_message(context: &str, error: impl std::fmt::Display) -> String {
    format!("{context} failed: {error}")
}
