use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use async_trait::async_trait;
use serde::Deserialize;
use ys_agent_core::{CoreError, CoreResult, MetricDefinition, MetricProvider, MetricStatus};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryFile {
    schema_version: u32,
    metrics: Vec<RawMetric>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMetric {
    id: String,
    version: u32,
    status: MetricStatus,
    description: String,
    source_relation: String,
    expression: String,
    time_column: String,
    allowed_dimensions: Vec<String>,
    owner: String,
    freshness_sla_seconds: Option<u64>,
}

#[derive(Debug, Clone)]
struct RegistryMetric {
    numeric_version: u32,
    definition: MetricDefinition,
}

#[derive(Debug, Clone)]
pub struct FileMetricRegistry {
    by_id: BTreeMap<String, Vec<RegistryMetric>>,
    aliases: BTreeMap<String, String>,
}

impl FileMetricRegistry {
    pub async fn load(path: impl AsRef<Path>) -> CoreResult<Self> {
        let bytes = tokio::fs::read(path.as_ref()).await.map_err(|error| {
            CoreError::validation(
                "metric_registry_read_failed",
                format!("cannot read metric registry: {error}"),
            )
        })?;
        Self::from_json_bytes(&bytes)
    }

    pub fn from_json_bytes(bytes: &[u8]) -> CoreResult<Self> {
        let file: RegistryFile = serde_json::from_slice(bytes).map_err(|error| {
            CoreError::validation(
                "invalid_metric_registry",
                format!("cannot parse metric registry: {error}"),
            )
        })?;
        if file.schema_version != 1 {
            return Err(CoreError::validation(
                "unsupported_metric_registry_version",
                format!(
                    "expected schema_version 1, received {}",
                    file.schema_version
                ),
            ));
        }

        let mut seen_versions = BTreeSet::new();
        let mut by_id = BTreeMap::<String, Vec<RegistryMetric>>::new();
        for raw in file.metrics {
            validate_metric(&raw)?;
            if !seen_versions.insert((raw.id.clone(), raw.version)) {
                return Err(CoreError::validation(
                    "duplicate_metric_version",
                    format!("duplicate metric {} version {}", raw.id, raw.version),
                ));
            }
            let definition = MetricDefinition {
                id: raw.id.clone(),
                version: raw.version.to_string(),
                status: raw.status,
                description: raw.description,
                source_relation: raw.source_relation,
                expression: raw.expression,
                time_column: raw.time_column,
                allowed_dimensions: raw.allowed_dimensions,
                owner: raw.owner,
                freshness_sla_seconds: raw.freshness_sla_seconds,
            };
            by_id.entry(raw.id).or_default().push(RegistryMetric {
                numeric_version: raw.version,
                definition,
            });
        }

        for versions in by_id.values_mut() {
            versions.sort_by_key(|metric| metric.numeric_version);
        }
        let aliases = build_aliases(by_id.keys())?;
        Ok(Self { by_id, aliases })
    }

    pub async fn resolve_active(&self, query: &str) -> CoreResult<Option<MetricDefinition>> {
        let exact_id = self.by_id.contains_key(query).then_some(query);
        let resolved_id = exact_id.or_else(|| {
            self.aliases
                .get(&query.trim().to_ascii_lowercase())
                .map(String::as_str)
        });
        let Some(id) = resolved_id else {
            return Ok(None);
        };
        Ok(self.by_id.get(id).and_then(|versions| {
            versions
                .iter()
                .rev()
                .find(|metric| metric.definition.status == MetricStatus::Active)
                .map(|metric| metric.definition.clone())
        }))
    }
}

#[async_trait]
impl MetricProvider for FileMetricRegistry {
    async fn get_metric(&self, metric_id: &str) -> CoreResult<Option<MetricDefinition>> {
        self.resolve_active(metric_id).await
    }

    async fn list_active_metrics(&self) -> CoreResult<Vec<MetricDefinition>> {
        let mut active = self
            .by_id
            .values()
            .filter_map(|versions| {
                versions
                    .iter()
                    .rev()
                    .find(|metric| metric.definition.status == MetricStatus::Active)
                    .map(|metric| metric.definition.clone())
            })
            .collect::<Vec<_>>();
        active.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(active)
    }
}

fn validate_metric(metric: &RawMetric) -> CoreResult<()> {
    validate_qualified_identifier(&metric.id, "metric id")?;
    validate_qualified_identifier(&metric.source_relation, "source relation")?;
    validate_identifier(&metric.time_column, "time column")?;
    for dimension in &metric.allowed_dimensions {
        validate_identifier(dimension, "dimension")?;
    }
    if metric.owner.trim().is_empty() {
        return Err(CoreError::validation(
            "metric_owner_missing",
            format!("metric {} has no owner", metric.id),
        ));
    }
    if metric.expression.trim().is_empty() {
        return Err(CoreError::validation(
            "metric_expression_empty",
            format!("metric {} has an empty expression", metric.id),
        ));
    }
    Ok(())
}

fn build_aliases<'a>(
    ids: impl Iterator<Item = &'a String>,
) -> CoreResult<BTreeMap<String, String>> {
    let mut aliases = BTreeMap::<String, String>::new();
    for id in ids {
        let alias = id.rsplit('.').next().unwrap_or(id).to_ascii_lowercase();
        if let Some(existing) = aliases.insert(alias.clone(), id.clone())
            && existing != *id
        {
            return Err(CoreError::validation(
                "ambiguous_metric_alias",
                format!("display alias {alias} matches {existing} and {id}"),
            ));
        }
    }
    Ok(aliases)
}

fn validate_qualified_identifier(value: &str, kind: &'static str) -> CoreResult<()> {
    if value
        .split('.')
        .any(|part| validate_identifier(part, kind).is_err())
    {
        return Err(CoreError::validation(
            "unsafe_metric_identifier",
            format!("{kind} {value:?} is unsafe"),
        ));
    }
    Ok(())
}

fn validate_identifier(value: &str, kind: &'static str) -> CoreResult<()> {
    let mut characters = value.chars();
    let first_is_safe = characters
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic());
    let rest_is_safe =
        characters.all(|character| character == '_' || character.is_ascii_alphanumeric());
    if !first_is_safe || !rest_is_safe {
        return Err(CoreError::validation(
            "unsafe_metric_identifier",
            format!("{kind} {value:?} is unsafe"),
        ));
    }
    Ok(())
}
