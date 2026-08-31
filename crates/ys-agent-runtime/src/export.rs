use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ys_agent_core::{
    ArtifactAccessContext, ArtifactAccessPurpose, ArtifactId, ArtifactKind, ArtifactMetadata,
    ArtifactRef, ArtifactStore, CommandId, CommandReceipt, CommandResultKind, CoreError,
    CoreResult, ExportFormat, RetentionPolicy, RuntimeCommandBatch, RuntimeStore, Sensitivity,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportDisposition {
    Allowed,
    Redacted,
    Denied,
}

impl ExportDisposition {
    pub fn error_code(self) -> Option<&'static str> {
        match self {
            Self::Denied => Some("artifact_export_denied"),
            Self::Allowed | Self::Redacted => None,
        }
    }
}

pub trait ExportPolicy: Send + Sync {
    fn decide(&self, sensitivity: Sensitivity) -> ExportDisposition;
}

#[derive(Debug, Default)]
pub struct DefaultExportPolicy;

impl ExportPolicy for DefaultExportPolicy {
    fn decide(&self, sensitivity: Sensitivity) -> ExportDisposition {
        match sensitivity {
            Sensitivity::Public | Sensitivity::Internal => ExportDisposition::Allowed,
            Sensitivity::Restricted => ExportDisposition::Denied,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedResultBody {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}

#[derive(Debug, Deserialize)]
struct QueryExportView {
    question: String,
    answer_summary: String,
    time_range: Option<Value>,
    executed_sql: Option<String>,
    #[serde(default)]
    warning_codes: Vec<String>,
    result_artifact: Option<ArtifactRef>,
}

pub fn format_export(
    format: ExportFormat,
    query_bytes: &[u8],
    result_bytes: Option<&[u8]>,
) -> CoreResult<(String, &'static str, Vec<u8>)> {
    match format {
        ExportFormat::Json => {
            serde_json::from_slice::<Value>(query_bytes).map_err(|error| {
                CoreError::validation(
                    "invalid_query_artifact",
                    format!("persisted Query Artifact is invalid JSON: {error}"),
                )
            })?;
            Ok(("application/json".to_owned(), "json", query_bytes.to_vec()))
        }
        ExportFormat::Csv => {
            let bytes = result_bytes.ok_or_else(|| {
                CoreError::validation(
                    "missing_result_artifact",
                    "CSV export requires a persisted Result Artifact",
                )
            })?;
            let result: PersistedResultBody = serde_json::from_slice(bytes).map_err(|error| {
                CoreError::validation(
                    "invalid_result_artifact",
                    format!("persisted Result Artifact has the wrong shape: {error}"),
                )
            })?;
            Ok((
                "text/csv".to_owned(),
                "csv",
                render_csv(&result).into_bytes(),
            ))
        }
        ExportFormat::Markdown => {
            let view: QueryExportView = serde_json::from_slice(query_bytes).map_err(|error| {
                CoreError::validation(
                    "invalid_query_artifact",
                    format!("persisted Query Artifact has the wrong shape: {error}"),
                )
            })?;
            Ok((
                "text/markdown; charset=utf-8".to_owned(),
                "md",
                render_markdown(&view).into_bytes(),
            ))
        }
    }
}

fn render_csv(result: &PersistedResultBody) -> String {
    let mut output = String::new();
    output.push_str(
        &result
            .columns
            .iter()
            .map(|value| csv_field(value))
            .collect::<Vec<_>>()
            .join(","),
    );
    output.push('\n');

    for row in &result.rows {
        let values = row
            .iter()
            .map(json_cell)
            .map(|value| csv_field(&value))
            .collect::<Vec<_>>();
        output.push_str(&values.join(","));
        output.push('\n');
    }

    output
}

fn json_cell(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => spreadsheet_safe_text(value),
        Value::Array(_) | Value::Object(_) => value.to_string(),
    }
}

fn spreadsheet_safe_text(value: &str) -> String {
    if value
        .chars()
        .next()
        .is_some_and(|character| matches!(character, '=' | '+' | '-' | '@'))
    {
        format!("'{value}")
    } else {
        value.to_owned()
    }
}

fn csv_field(value: &str) -> String {
    if value
        .chars()
        .any(|character| matches!(character, ',' | '"' | '\n' | '\r'))
    {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

fn render_markdown(view: &QueryExportView) -> String {
    let time_range = view
        .time_range
        .as_ref()
        .map(Value::to_string)
        .unwrap_or_else(|| "Not applicable".to_owned());
    let warnings = if view.warning_codes.is_empty() {
        "None".to_owned()
    } else {
        view.warning_codes.join(", ")
    };
    let sql = view.executed_sql.as_deref().unwrap_or("Not executed");

    format!(
        "# Query Artifact\n\n## Question\n\n{}\n\n## Answer\n\n{}\n\n## Time range\n\n{}\n\n## Warnings\n\n{}\n\n## SQL\n\n```sql\n{}\n```\n",
        markdown_text(&view.question),
        markdown_text(&view.answer_summary),
        markdown_text(&time_range),
        markdown_text(&warnings),
        sql.replace("```", "` ` `"),
    )
}

fn markdown_text(value: &str) -> String {
    value.replace('<', "&lt;").replace('>', "&gt;")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenExport {
    pub storage_uri: String,
    pub content_hash: String,
    pub size_bytes: u64,
}

#[async_trait]
pub trait ExportWriter: Send + Sync {
    async fn write(
        &self,
        source_artifact_id: ArtifactId,
        extension: &str,
        bytes: &[u8],
    ) -> CoreResult<WrittenExport>;
}

#[async_trait]
pub trait ArtifactExportService: Send + Sync {
    async fn export(
        &self,
        command_id: CommandId,
        artifact_id: &ArtifactId,
        format: ExportFormat,
        access: ArtifactAccessContext,
    ) -> CoreResult<ArtifactMetadata>;
}

pub struct ArtifactExporter {
    runtime_store: Arc<dyn RuntimeStore>,
    artifact_store: Arc<dyn ArtifactStore>,
    writer: Arc<dyn ExportWriter>,
    policy: Arc<dyn ExportPolicy>,
}

impl ArtifactExporter {
    pub fn new(
        runtime_store: Arc<dyn RuntimeStore>,
        artifact_store: Arc<dyn ArtifactStore>,
        writer: Arc<dyn ExportWriter>,
        policy: Arc<dyn ExportPolicy>,
    ) -> Self {
        Self {
            runtime_store,
            artifact_store,
            writer,
            policy,
        }
    }

    async fn read_body(
        &self,
        metadata: ArtifactMetadata,
        access: &ArtifactAccessContext,
    ) -> CoreResult<Vec<u8>> {
        self.artifact_store
            .get(&ArtifactRef::new(metadata), access)
            .await
    }
}

#[async_trait]
impl ArtifactExportService for ArtifactExporter {
    async fn export(
        &self,
        command_id: CommandId,
        artifact_id: &ArtifactId,
        format: ExportFormat,
        mut access: ArtifactAccessContext,
    ) -> CoreResult<ArtifactMetadata> {
        let fingerprint = crate::service::command_fingerprint(
            "export_artifact",
            serde_json::json!({
                "artifact_id": artifact_id,
                "format": format,
                "workspace_id": access.workspace_id,
                "principal_id": access.principal_id,
            }),
        )?;
        if let Some(receipt) = self.runtime_store.load_command(&command_id).await? {
            if receipt.command_fingerprint != fingerprint {
                return Err(CoreError::IdempotencyConflict {
                    command_id: command_id.to_string(),
                });
            }
            let exported_id = receipt.artifact_id.ok_or_else(|| {
                CoreError::validation(
                    "invalid_export_receipt",
                    "export receipt has no Artifact ID",
                )
            })?;
            return self.runtime_store.load_artifact(&exported_id).await;
        }

        access.purpose = ArtifactAccessPurpose::Export;
        let source = self.runtime_store.load_artifact(artifact_id).await?;

        if self.policy.decide(source.sensitivity) == ExportDisposition::Denied {
            return Err(CoreError::validation(
                "artifact_export_denied",
                "Artifact Policy denied this export",
            ));
        }

        let query_bytes = self.read_body(source.clone(), &access).await?;
        let query_view: QueryExportView =
            serde_json::from_slice(&query_bytes).map_err(|error| {
                CoreError::validation(
                    "invalid_query_artifact",
                    format!("persisted Query Artifact has the wrong shape: {error}"),
                )
            })?;

        let result_bytes = match query_view.result_artifact {
            Some(reference) => Some(self.read_body(reference.metadata, &access).await?),
            None => None,
        };
        let (media_type, extension, output) =
            format_export(format, &query_bytes, result_bytes.as_deref())?;
        let written = self.writer.write(*artifact_id, extension, &output).await?;

        let metadata = ArtifactMetadata::builder(source.sensitivity)
            .workspace_id(source.workspace_id)
            .task_id(source.task_id)
            .run_id(source.run_id)
            .kind(ArtifactKind::Export)
            .media_type(media_type)
            .content_hash(written.content_hash)
            .size_bytes(written.size_bytes)
            .storage_uri(written.storage_uri)
            .owner(access.principal_id)
            .retention_policy(RetentionPolicy::Days { days: 7 })
            .build()?;
        let receipt = CommandReceipt {
            command_id,
            command_fingerprint: fingerprint.clone(),
            result_kind: CommandResultKind::ArtifactExported,
            session_id: None,
            task_id: Some(source.task_id),
            run_id: Some(source.run_id),
            artifact_id: Some(metadata.id),
        };
        let stored = self
            .runtime_store
            .commit_command(RuntimeCommandBatch {
                command_id,
                command_fingerprint: fingerprint,
                receipt,
                new_session: None,
                new_task: None,
                new_run_snapshot: None,
                new_artifact: Some(metadata.clone()),
                pending_events: Vec::new(),
                snapshot_update: None,
            })
            .await?;
        let stored_id = stored.artifact_id.ok_or_else(|| {
            CoreError::validation(
                "invalid_export_receipt",
                "stored export receipt has no Artifact ID",
            )
        })?;
        self.runtime_store.load_artifact(&stored_id).await
    }
}
