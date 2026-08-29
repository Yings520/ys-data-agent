use std::collections::BTreeMap;
use std::path::Path;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ys_agent_core::{
    ContextEvidence, ContextSourceType, CoreError, CoreResult, InstructionTrust,
    QueryContextProvider, Sensitivity,
};

#[derive(Debug, Deserialize)]
struct RawManifest {
    metadata: RawMetadata,
    #[serde(default)]
    nodes: BTreeMap<String, RawResource>,
    #[serde(default)]
    sources: BTreeMap<String, RawResource>,
}

#[derive(Debug, Deserialize)]
struct RawMetadata {
    dbt_schema_version: String,
    generated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct RawResource {
    unique_id: String,
    resource_type: String,
    database: Option<String>,
    schema: String,
    name: String,
    alias: Option<String>,
    identifier: Option<String>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    columns: BTreeMap<String, RawColumn>,
    #[serde(default)]
    depends_on: RawDependsOn,
    checksum: Option<RawChecksum>,
}

#[derive(Debug, Deserialize, Serialize)]
struct RawColumn {
    name: Option<String>,
    #[serde(default)]
    description: String,
    data_type: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RawDependsOn {
    #[serde(default)]
    nodes: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct RawChecksum {
    name: String,
    checksum: String,
}

#[derive(Debug, Clone)]
struct ManifestEntry {
    unique_id: String,
    resource_type: String,
    database: Option<String>,
    schema: String,
    name: String,
    relation_name: String,
    description: String,
    columns: BTreeMap<String, ManifestColumn>,
    depends_on: Vec<String>,
    checksum: Option<ManifestChecksum>,
}

#[derive(Debug, Clone, Serialize)]
struct ManifestColumn {
    name: String,
    description: String,
    data_type: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ManifestChecksum {
    name: String,
    checksum: String,
}

#[derive(Debug, Serialize)]
struct EvidenceDocument<'a> {
    unique_id: &'a str,
    resource_type: &'a str,
    database: Option<&'a str>,
    schema: &'a str,
    name: &'a str,
    relation_name: &'a str,
    description: &'a str,
    columns: &'a BTreeMap<String, ManifestColumn>,
    depends_on: &'a [String],
    checksum: Option<&'a ManifestChecksum>,
}

#[derive(Debug, Clone)]
pub struct DbtManifestAdapter {
    dbt_schema_version: String,
    generated_at: DateTime<Utc>,
    content_version: String,
    entries: BTreeMap<String, ManifestEntry>,
}

impl DbtManifestAdapter {
    pub async fn load(path: impl AsRef<Path>) -> CoreResult<Self> {
        let bytes = tokio::fs::read(path.as_ref()).await.map_err(|error| {
            CoreError::validation(
                "dbt_manifest_read_failed",
                format!("cannot read dbt manifest: {error}"),
            )
        })?;
        Self::from_json_bytes(&bytes)
    }

    pub fn from_json_bytes(bytes: &[u8]) -> CoreResult<Self> {
        let raw: RawManifest = serde_json::from_slice(bytes).map_err(|error| {
            CoreError::validation(
                "invalid_dbt_manifest",
                format!("cannot parse dbt manifest: {error}"),
            )
        })?;
        if raw.metadata.dbt_schema_version.trim().is_empty() {
            return Err(CoreError::validation(
                "dbt_schema_version_missing",
                "dbt metadata.dbt_schema_version is required",
            ));
        }

        let mut entries = BTreeMap::new();
        for (map_key, resource) in raw.nodes {
            if resource.resource_type != "model" {
                continue;
            }
            let entry = normalize_resource(&map_key, resource)?;
            let unique_id = entry.unique_id.clone();
            if entries.insert(unique_id.clone(), entry).is_some() {
                return Err(CoreError::validation(
                    "duplicate_dbt_resource",
                    format!("dbt resource {unique_id} appears more than once"),
                ));
            }
        }
        for (map_key, resource) in raw.sources {
            if resource.resource_type != "source" {
                continue;
            }
            let entry = normalize_resource(&map_key, resource)?;
            let unique_id = entry.unique_id.clone();
            if entries.insert(unique_id.clone(), entry).is_some() {
                return Err(CoreError::validation(
                    "duplicate_dbt_resource",
                    format!("dbt resource {unique_id} appears more than once"),
                ));
            }
        }

        let digest = Sha256::digest(bytes);
        let content_version = format!(
            "sha256:{}",
            digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        Ok(Self {
            dbt_schema_version: raw.metadata.dbt_schema_version,
            generated_at: raw.metadata.generated_at,
            content_version,
            entries,
        })
    }

    pub fn dbt_schema_version(&self) -> &str {
        &self.dbt_schema_version
    }

    pub async fn find_model(&self, unique_id: &str) -> CoreResult<ContextEvidence> {
        let entry = self
            .entries
            .get(unique_id)
            .ok_or_else(|| CoreError::NotFound {
                entity: "dbt_model",
                id: unique_id.to_owned(),
            })?;
        if entry.resource_type != "model" {
            return Err(CoreError::validation(
                "dbt_resource_not_model",
                format!("{unique_id} is not a dbt model"),
            ));
        }
        self.to_evidence(entry)
    }

    pub async fn find_relation(&self, relation: &str) -> CoreResult<Option<ContextEvidence>> {
        let relation = relation.to_ascii_lowercase();
        let mut matches = self.entries.values().filter(|entry| {
            entry.relation_name.to_ascii_lowercase() == relation
                || entry
                    .relation_name
                    .to_ascii_lowercase()
                    .ends_with(&format!(".{relation}"))
                || entry.name.to_ascii_lowercase() == relation
        });
        let first = matches.next();
        if first.is_some() && matches.next().is_some() {
            return Err(CoreError::validation(
                "ambiguous_dbt_relation",
                format!("relation {relation} matches more than one dbt resource"),
            ));
        }
        first.map(|entry| self.to_evidence(entry)).transpose()
    }

    fn to_evidence(&self, entry: &ManifestEntry) -> CoreResult<ContextEvidence> {
        let document = EvidenceDocument {
            unique_id: &entry.unique_id,
            resource_type: &entry.resource_type,
            database: entry.database.as_deref(),
            schema: &entry.schema,
            name: &entry.name,
            relation_name: &entry.relation_name,
            description: &entry.description,
            columns: &entry.columns,
            depends_on: &entry.depends_on,
            checksum: entry.checksum.as_ref(),
        };
        let text = serde_json::to_string_pretty(&document).map_err(|error| {
            CoreError::validation("dbt_evidence_serialization_failed", error.to_string())
        })?;
        Ok(ContextEvidence {
            source: format!("dbt://{}", entry.unique_id),
            source_type: ContextSourceType::DbtManifest,
            version: self.content_version.clone(),
            observed_at: self.generated_at.to_owned(),
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
}

#[async_trait]
impl QueryContextProvider for DbtManifestAdapter {
    async fn load_evidence(&self, query: &str) -> CoreResult<Vec<ContextEvidence>> {
        if let Some(evidence) = self.find_relation(query).await? {
            return Ok(vec![evidence]);
        }

        let query = query.trim().to_ascii_lowercase();
        let mut matches = self
            .entries
            .values()
            .filter(|entry| {
                entry.unique_id.to_ascii_lowercase().contains(&query)
                    || entry.name.to_ascii_lowercase().contains(&query)
                    || entry.description.to_ascii_lowercase().contains(&query)
            })
            .map(|entry| self.to_evidence(entry))
            .collect::<CoreResult<Vec<_>>>()?;
        matches.sort_by(|left, right| left.source.cmp(&right.source));
        Ok(matches)
    }
}

fn normalize_resource(map_key: &str, raw: RawResource) -> CoreResult<ManifestEntry> {
    if map_key != raw.unique_id {
        return Err(CoreError::validation(
            "dbt_identity_mismatch",
            format!("manifest key {map_key} does not match {}", raw.unique_id),
        ));
    }
    let physical_name = raw
        .alias
        .as_deref()
        .or(raw.identifier.as_deref())
        .unwrap_or(&raw.name)
        .to_owned();
    let relation_name = match raw.database.as_deref() {
        Some(database) if !database.is_empty() => {
            format!("{database}.{}.{}", raw.schema, physical_name)
        }
        _ => format!("{}.{}", raw.schema, physical_name),
    };
    let columns = raw
        .columns
        .into_iter()
        .map(|(key, column)| {
            let name = column.name.unwrap_or_else(|| key.clone());
            (
                key,
                ManifestColumn {
                    name,
                    description: column.description,
                    data_type: column.data_type,
                },
            )
        })
        .collect();
    let checksum = raw.checksum.map(|checksum| ManifestChecksum {
        name: checksum.name,
        checksum: checksum.checksum,
    });

    Ok(ManifestEntry {
        unique_id: raw.unique_id,
        resource_type: raw.resource_type,
        database: raw.database,
        schema: raw.schema,
        name: raw.name,
        relation_name,
        description: raw.description,
        columns,
        depends_on: raw.depends_on.nodes,
        checksum,
    })
}

fn estimate_tokens(text: &str) -> u32 {
    let bytes = u32::try_from(text.len()).unwrap_or(u32::MAX);
    bytes.saturating_add(3) / 4
}
