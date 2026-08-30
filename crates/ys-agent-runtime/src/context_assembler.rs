use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::RwLock;
use ys_agent_core::{
    ArtifactKind, ArtifactMetadata, ArtifactStore, ContextEvidence, ContextManifest,
    ContextOmission, ContextSourceType, CoreError, CoreResult, EventActor, InstructionTrust,
    MetricDefinition, MetricProvider, MetricStatus, ModelMessage, ModelRequest, ModelRole,
    PendingRunEvent, PutArtifact, QueryContextProvider, RetentionPolicy, RunEventKind, RunId,
    Sensitivity, TaskId, ToolSpec, WorkspaceId,
};

use crate::{
    tools::QueryPhase,
    workflow::query::{QUERY_SYSTEM_PROMPT_VERSION, query_system_instructions},
};

#[derive(Debug, Clone)]
pub struct ToolViewSnapshot {
    version: String,
    tools: Vec<ToolSpec>,
}

impl ToolViewSnapshot {
    pub fn new(version: impl Into<String>, tools: Vec<ToolSpec>) -> CoreResult<Self> {
        let version = version.into();
        if version.trim().is_empty() {
            return Err(CoreError::validation(
                "tool_view_version_missing",
                "ToolView snapshot needs a content hash/version",
            ));
        }
        let mut names = BTreeSet::new();
        for tool in &tools {
            if !names.insert(tool.name.clone()) {
                return Err(CoreError::validation(
                    "duplicate_tool_in_view",
                    format!("ToolView contains {} more than once", tool.name),
                ));
            }
        }
        Ok(Self { version, tools })
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn tools(&self) -> &[ToolSpec] {
        &self.tools
    }
}

#[derive(Debug, Clone)]
pub struct RecentTaskSummary {
    pub text: String,
    pub explicitly_relevant: bool,
}

#[derive(Debug, Clone)]
pub struct ContextAssemblyRequest {
    pub task_goal: String,
    pub query: String,
    pub token_budget: u32,
    pub schema_ttl: Duration,
    pub requires_schema: bool,
    pub requires_freshness: bool,
    pub recent_task_summary: Option<RecentTaskSummary>,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetrievalNeed {
    ObservedSchema,
    Freshness,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssembledContext {
    pub manifest: ContextManifest,
    pub retrieval_needs: Vec<RetrievalNeed>,
}

#[derive(Debug)]
struct RankedEvidence {
    rank: u8,
    evidence: ContextEvidence,
}

pub struct ContextAssembler {
    metrics: Arc<dyn MetricProvider>,
    dbt: Arc<dyn QueryContextProvider>,
    run_evidence: Arc<dyn QueryContextProvider>,
}

impl ContextAssembler {
    pub fn new(
        metrics: Arc<dyn MetricProvider>,
        dbt: Arc<dyn QueryContextProvider>,
        run_evidence: Arc<dyn QueryContextProvider>,
    ) -> Self {
        Self {
            metrics,
            dbt,
            run_evidence,
        }
    }

    pub async fn assemble(
        &self,
        request: &ContextAssemblyRequest,
        tool_view: &ToolViewSnapshot,
    ) -> CoreResult<AssembledContext> {
        let mut candidates = Vec::<RankedEvidence>::new();
        let mut omissions = Vec::<ContextOmission>::new();
        let metric = self.metrics.get_metric(&request.query).await?;
        let relation_search = if let Some(metric) = metric.as_ref() {
            if metric.status != MetricStatus::Active {
                return Err(CoreError::validation(
                    "inactive_metric_reached_assembler",
                    format!("metric {} is not Active", metric.id),
                ));
            }

            candidates.push(RankedEvidence {
                rank: 0,
                evidence: metric_evidence(metric, request.now.to_owned())?,
            });

            metric.source_relation.as_str()
        } else {
            request.query.as_str()
        };
        for evidence in self.dbt.load_evidence(relation_search).await? {
            if evidence.source_type == ContextSourceType::DbtManifest {
                candidates.push(RankedEvidence { rank: 1, evidence });
            }
        }

        let mut has_valid_schema = false;
        let mut has_freshness = false;
        for evidence in self.run_evidence.load_evidence(relation_search).await? {
            match evidence.source_type {
                ContextSourceType::ObservedSchema => {
                    if evidence_is_within_ttl(&evidence, request.now.to_owned(), request.schema_ttl)
                    {
                        if evidence.sensitivity == Sensitivity::Restricted {
                            omissions.push(ContextOmission {
                                uri: evidence.source,
                                reason: "restricted_evidence_not_model_safe".to_owned(),
                            });
                        } else {
                            has_valid_schema = true;
                            candidates.push(RankedEvidence { rank: 2, evidence });
                        }
                    } else {
                        omissions.push(ContextOmission {
                            uri: evidence.source,
                            reason: "expired".to_owned(),
                        });
                    }
                }
                ContextSourceType::Freshness => {
                    if evidence.sensitivity == Sensitivity::Restricted {
                        omissions.push(ContextOmission {
                            uri: evidence.source,
                            reason: "restricted_evidence_not_model_safe".to_owned(),
                        });
                    } else {
                        has_freshness = true;
                        candidates.push(RankedEvidence { rank: 2, evidence });
                    }
                }
                ContextSourceType::TaskSummary
                | ContextSourceType::MetricRegistry
                | ContextSourceType::DbtManifest
                | ContextSourceType::Fixture => {
                    omissions.push(ContextOmission {
                        uri: evidence.source,
                        reason: "source_not_ranked_here".to_owned(),
                    });
                }
            }
        }

        candidates.sort_by(|left, right| {
            left.rank
                .cmp(&right.rank)
                .then_with(|| left.evidence.source.cmp(&right.evidence.source))
                .then_with(|| left.evidence.version.cmp(&right.evidence.version))
        });
        let mut manifest = ContextManifest::empty(request.token_budget);
        manifest.tool_view_version = tool_view.version().to_owned();
        manifest.omitted = omissions;
        let mut seen = BTreeSet::<(String, String)>::new();

        for mut candidate in candidates {
            let identity = (
                candidate.evidence.source.clone(),
                candidate.evidence.version.clone(),
            );
            if !seen.insert(identity) {
                manifest.omitted.push(ContextOmission {
                    uri: candidate.evidence.source,
                    reason: "duplicate".to_owned(),
                });
                continue;
            }
            if candidate.evidence.instruction_trust != InstructionTrust::UntrustedData {
                return Err(CoreError::validation(
                    "invalid_instruction_trust",
                    "all v0.2 Evidence must be UntrustedData",
                ));
            }

            if candidate.evidence.sensitivity == Sensitivity::Restricted {
                manifest.omitted.push(ContextOmission {
                    uri: candidate.evidence.source,
                    reason: "restricted_evidence_not_model_safe".to_owned(),
                });
                continue;
            }
            let token_cost = estimate_tokens(&candidate.evidence.text);
            candidate.evidence.token_cost = token_cost;
            if manifest.tokens_used.saturating_add(token_cost) > manifest.token_budget {
                manifest.omitted.push(ContextOmission {
                    uri: candidate.evidence.source,
                    reason: "token_budget".to_owned(),
                });
                continue;
            }
            manifest.tokens_used += token_cost;
            manifest.included.push(candidate.evidence);
        }
        if let Some(summary) = &request.recent_task_summary {
            if !summary.explicitly_relevant
                || !text_is_relevant(&summary.text, &request.task_goal, &request.query)
            {
                manifest.omitted.push(ContextOmission {
                    uri: "task-summary://recent".to_owned(),
                    reason: "summary_not_explicitly_relevant".to_owned(),
                });
            } else {
                let cost = estimate_tokens(&summary.text);
                if manifest.tokens_used.saturating_add(cost) > manifest.token_budget {
                    manifest.omitted.push(ContextOmission {
                        uri: "task-summary://recent".to_owned(),
                        reason: "token_budget".to_owned(),
                    });
                } else {
                    manifest.tokens_used += cost;
                    manifest.summaries.push(summary.text.clone());
                }
            }
        }

        let mut retrieval_needs = Vec::new();
        if request.requires_schema && !has_valid_schema {
            retrieval_needs.push(RetrievalNeed::ObservedSchema);
        }
        if request.requires_freshness && !has_freshness {
            retrieval_needs.push(RetrievalNeed::Freshness);
        }
        Ok(AssembledContext {
            manifest,
            retrieval_needs,
        })
    }
}

fn metric_evidence(
    metric: &MetricDefinition,
    observed_at: DateTime<Utc>,
) -> CoreResult<ContextEvidence> {
    let text = serde_json::to_string_pretty(metric).map_err(|error| {
        CoreError::validation("metric_evidence_serialization_failed", error.to_string())
    })?;
    Ok(ContextEvidence {
        source: format!("metric://{}@{}", metric.id, metric.version),
        source_type: ContextSourceType::MetricRegistry,
        version: metric.version.clone(),
        observed_at,
        freshness: None,
        owner: None,
        acl: vec!["data_query".to_owned()],
        sensitivity: Sensitivity::Internal,
        confidence: 1.0,
        token_cost: estimate_tokens(&text),
        instruction_trust: InstructionTrust::UntrustedData,
        text,
    })
}

fn estimate_tokens(text: &str) -> u32 {
    let bytes = u32::try_from(text.len()).unwrap_or(u32::MAX);
    bytes.saturating_add(3) / 4
}

fn evidence_is_within_ttl(evidence: &ContextEvidence, now: DateTime<Utc>, ttl: Duration) -> bool {
    let Ok(ttl) = chrono::Duration::from_std(ttl) else {
        return false;
    };
    let age = now.signed_duration_since(evidence.observed_at);
    age >= chrono::Duration::zero() && age <= ttl
}

fn text_is_relevant(summary: &str, goal: &str, query: &str) -> bool {
    let summary_words = words(summary);
    let subject_words = words(&format!("{goal} {query}"));
    summary_words.intersection(&subject_words).next().is_some()
}

fn words(text: &str) -> BTreeSet<String> {
    text.split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .filter(|word| word.len() >= 3)
        .map(str::to_ascii_lowercase)
        .collect()
}

#[derive(Debug, Default)]
pub struct InMemoryQueryContextProvider {
    evidence: RwLock<Vec<ContextEvidence>>,
}

impl InMemoryQueryContextProvider {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn insert(&self, evidence: ContextEvidence) {
        self.evidence.write().await.push(evidence);
    }
}

#[async_trait]
impl QueryContextProvider for InMemoryQueryContextProvider {
    async fn load_evidence(&self, query: &str) -> CoreResult<Vec<ContextEvidence>> {
        let query = query.trim().to_ascii_lowercase();
        let mut matches = self
            .evidence
            .read()
            .await
            .iter()
            .filter(|evidence| {
                query.is_empty()
                    || evidence.source.to_ascii_lowercase().contains(&query)
                    || evidence.text.to_ascii_lowercase().contains(&query)
            })
            .cloned()
            .collect::<Vec<_>>();
        matches.sort_by(|left, right| {
            left.source
                .cmp(&right.source)
                .then_with(|| left.version.cmp(&right.version))
        });
        Ok(matches)
    }
}

#[derive(Debug, serde::Serialize)]
struct PromptEvidenceBlock<'a> {
    source: &'a str,
    source_type: ContextSourceType,
    version: &'a str,
    instruction_trust: InstructionTrust,
    text: &'a str,
}

#[derive(Debug, Clone)]
pub struct PromptBuilder {
    model: String,
}

impl PromptBuilder {
    pub const VERSION: &'static str = QUERY_SYSTEM_PROMPT_VERSION;

    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
        }
    }

    pub fn build(
        &self,
        task_goal: &str,
        phase: QueryPhase,
        manifest: &ContextManifest,
        tool_view: &ToolViewSnapshot,
    ) -> CoreResult<ModelRequest> {
        if self.model.trim().is_empty() {
            return Err(CoreError::validation(
                "model_name_missing",
                "PromptBuilder needs a model name",
            ));
        }
        if manifest.tool_view_version != tool_view.version() {
            return Err(CoreError::validation(
                "tool_view_version_mismatch",
                "ContextManifest and ToolView snapshot do not match",
            ));
        }
        validate_manifest_budget(manifest)?;

        let mut messages = vec![
            ModelMessage {
                role: ModelRole::System,
                content: query_system_instructions(phase),
                tool_call_id: None,
                name: None,
            },
            ModelMessage {
                role: ModelRole::User,
                content: format!("TASK_GOAL:\n{task_goal}"),
                tool_call_id: None,
                name: None,
            },
        ];

        for evidence in &manifest.included {
            if evidence.instruction_trust != InstructionTrust::UntrustedData {
                return Err(CoreError::validation(
                    "invalid_instruction_trust",
                    "Evidence must be UntrustedData",
                ));
            }
            if evidence.sensitivity == Sensitivity::Restricted {
                return Err(CoreError::validation(
                    "restricted_evidence_in_prompt",
                    format!(
                        "restricted Evidence {} cannot enter prompt",
                        evidence.source
                    ),
                ));
            }
            let block = PromptEvidenceBlock {
                source: &evidence.source,
                source_type: evidence.source_type,
                version: &evidence.version,
                instruction_trust: evidence.instruction_trust,
                text: &evidence.text,
            };
            let content = serde_json::to_string(&block).map_err(|error| {
                CoreError::validation("prompt_evidence_serialization_failed", error.to_string())
            })?;
            messages.push(ModelMessage {
                role: ModelRole::User,
                content: format!("UNTRUSTED_EVIDENCE_JSON:\n{content}"),
                tool_call_id: None,
                name: None,
            });
        }

        for summary in &manifest.summaries {
            messages.push(ModelMessage {
                role: ModelRole::User,
                content: format!("UNTRUSTED_TASK_SUMMARY:\n{summary}"),
                tool_call_id: None,
                name: None,
            });
        }

        Ok(ModelRequest {
            model: self.model.clone(),
            messages,
            tools: tool_view.tools().to_vec(),
            context_manifest: manifest.clone(),
            temperature: Some(0.0),
        })
    }
}

fn validate_manifest_budget(manifest: &ContextManifest) -> CoreResult<()> {
    let evidence_tokens = manifest.included.iter().fold(0u32, |total, evidence| {
        total.saturating_add(estimate_tokens(&evidence.text))
    });
    let summary_tokens = manifest.summaries.iter().fold(0u32, |total, summary| {
        total.saturating_add(estimate_tokens(summary))
    });
    let calculated = evidence_tokens.saturating_add(summary_tokens);
    if calculated != manifest.tokens_used {
        return Err(CoreError::validation(
            "context_token_accounting_mismatch",
            format!(
                "manifest records {} tokens but deterministic estimate is {calculated}",
                manifest.tokens_used
            ),
        ));
    }
    if calculated > manifest.token_budget {
        return Err(CoreError::validation(
            "context_budget_exceeded",
            "ContextManifest exceeds its token budget",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct PersistContextIdentity {
    pub workspace_id: WorkspaceId,
    pub task_id: TaskId,
    pub run_id: RunId,
}

#[derive(Debug, Clone)]
pub struct PreparedContextManifest {
    pub metadata: ArtifactMetadata,
    pub artifact_created: PendingRunEvent,
    pub model_requested: PendingRunEvent,
}

pub struct ContextManifestArtifactWriter {
    store: Arc<dyn ArtifactStore>,
}

impl ContextManifestArtifactWriter {
    pub fn new(store: Arc<dyn ArtifactStore>) -> Self {
        Self { store }
    }

    pub async fn persist(
        &self,
        manifest: &ContextManifest,
        identity: PersistContextIdentity,
        model_call_id: impl Into<String>,
        prompt_version: impl Into<String>,
    ) -> CoreResult<PreparedContextManifest> {
        validate_manifest_budget(manifest)?;
        if manifest.tool_view_version.trim().is_empty() || manifest.tool_view_version == "v0" {
            return Err(CoreError::validation(
                "tool_view_version_missing",
                "ContextManifest needs the actual ToolView content hash",
            ));
        }
        for evidence in &manifest.included {
            if evidence.sensitivity == Sensitivity::Restricted {
                return Err(CoreError::validation(
                    "restricted_evidence_in_manifest",
                    format!("restricted Evidence {} is not model-safe", evidence.source),
                ));
            }
            if evidence.instruction_trust != InstructionTrust::UntrustedData {
                return Err(CoreError::validation(
                    "invalid_instruction_trust",
                    "ContextManifest contains non-data instructions",
                ));
            }
        }

        let prompt_version = prompt_version.into();
        if prompt_version.trim().is_empty() {
            return Err(CoreError::validation(
                "prompt_version_missing",
                "ModelRequested needs a prompt version",
            ));
        }
        let model_call_id = model_call_id.into();
        if model_call_id.trim().is_empty() {
            return Err(CoreError::validation(
                "model_call_id_missing",
                "ModelRequested needs a model call ID",
            ));
        }
        let bytes = serde_json::to_vec(manifest).map_err(|error| {
            CoreError::validation("context_manifest_serialization_failed", error.to_string())
        })?;
        let metadata = self
            .store
            .put(PutArtifact {
                workspace_id: identity.workspace_id,
                task_id: identity.task_id,
                run_id: identity.run_id,
                kind: ArtifactKind::ContextManifest,
                media_type: "application/json".to_owned(),
                bytes,
                sensitivity: Sensitivity::Internal,
                owner: None,
                retention_policy: Some(RetentionPolicy::Session),
                expires_at: None,
                producer_step_id: None,
            })
            .await?;
        let artifact_created = PendingRunEvent {
            actor: EventActor::System,
            kind: RunEventKind::ArtifactCreated {
                artifact: metadata.clone(),
            },
        };
        let model_requested = PendingRunEvent {
            actor: EventActor::System,
            kind: RunEventKind::ModelRequested {
                model_call_id,
                context_manifest_id: metadata.id,
                tool_view_version: manifest.tool_view_version.clone(),
                prompt_version,
            },
        };

        Ok(PreparedContextManifest {
            metadata,
            artifact_created,
            model_requested,
        })
    }
}
