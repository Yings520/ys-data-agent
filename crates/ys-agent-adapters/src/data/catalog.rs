use std::{collections::BTreeMap, num::NonZeroU64, sync::Arc};

use async_trait::async_trait;
use ys_agent_core::*;

use super::sqlite::{SqliteConnector, ds_failure};

/// Metadata and its executable factory are one registration, never parallel registries.
pub struct ConnectorRegistration {
    pub descriptor: ConnectorDescriptor,
    pub factory: Arc<dyn ConnectorFactory>,
}

pub struct BuiltinConnectorCatalog {
    registrations: BTreeMap<(String, String), ConnectorRegistration>,
}

impl BuiltinConnectorCatalog {
    /// The remaining concrete drivers are injected by the application composition root.
    /// Constructing this catalog never opens a file, resolves a secret, or makes a connection.
    pub fn new(
        postgres: Arc<dyn ConnectorFactory>,
        duckdb: Arc<dyn ConnectorFactory>,
    ) -> DsResult<Self> {
        Self::from_registrations(vec![
            ConnectorRegistration {
                descriptor: builtin_descriptor("sqlite")?,
                factory: Arc::new(SqliteConnectorFactory),
            },
            ConnectorRegistration {
                descriptor: builtin_descriptor("postgres")?,
                factory: postgres,
            },
            ConnectorRegistration {
                descriptor: builtin_descriptor("duckdb")?,
                factory: duckdb,
            },
        ])
    }

    pub fn from_registrations(registrations: Vec<ConnectorRegistration>) -> DsResult<Self> {
        let mut entries = BTreeMap::new();
        for entry in registrations {
            let descriptor = &entry.descriptor;
            if descriptor.schema_version != 1
                || descriptor.contract_version != 1
                || descriptor.config_version == 0
                || descriptor.fields.is_empty()
                || (descriptor.support == ConnectorSupport::Supported
                    && descriptor.release_evidence.is_none())
                || !validate_datasource_fields(&descriptor.fields, &BTreeMap::new(), false, false)
                    .is_empty()
                || !descriptor.capability.supports_governed_query()
                || descriptor.capability.max_concurrency as u64 > descriptor.max_connections.get()
            {
                return Err(ds_failure(DsErrorCode::ConfigIncompatible));
            }
            let key = (
                descriptor.adapter_id.as_str().to_owned(),
                descriptor.adapter_version.as_str().to_owned(),
            );
            if entries.insert(key, entry).is_some() {
                return Err(ds_failure(DsErrorCode::Conflict));
            }
        }
        if entries.is_empty() {
            return Err(ds_failure(DsErrorCode::CapabilityMissing));
        }
        Ok(Self {
            registrations: entries,
        })
    }
}

impl ConnectorCatalog for BuiltinConnectorCatalog {
    fn descriptors(&self) -> DsResult<Vec<ConnectorDescriptor>> {
        Ok(self
            .registrations
            .values()
            .map(|entry| entry.descriptor.clone())
            .collect())
    }

    fn factory(
        &self,
        id: &AdapterId,
        version: &AdapterVersion,
    ) -> DsResult<Arc<dyn ConnectorFactory>> {
        let entry = self
            .registrations
            .get(&(id.as_str().to_owned(), version.as_str().to_owned()))
            .ok_or_else(|| ds_failure(DsErrorCode::ConfigIncompatible))?;
        if matches!(
            entry.descriptor.support,
            ConnectorSupport::Incompatible | ConnectorSupport::Unsupported
        ) {
            return Err(ds_failure(DsErrorCode::CapabilityMissing));
        }
        Ok(entry.factory.clone())
    }
}

pub fn builtin_descriptor(adapter: &str) -> DsResult<ConnectorDescriptor> {
    let (name, fields, connections) = match adapter {
        "sqlite" => (
            "SQLite",
            vec![field(
                "database_path",
                "Database file",
                FieldInput::ExistingFile,
                None,
            )],
            1,
        ),
        "duckdb" => (
            "DuckDB",
            vec![field(
                "database_path",
                "Database file",
                FieldInput::ExistingFile,
                None,
            )],
            1,
        ),
        "postgres" => (
            "PostgreSQL",
            vec![
                field("host", "Host", FieldInput::Text, None),
                field(
                    "port",
                    "Port",
                    FieldInput::Integer { min: 1, max: 65535 },
                    Some(FieldValue::Integer(5432)),
                ),
                field("database", "Database", FieldInput::Text, None),
                field(
                    "schema",
                    "Schema",
                    FieldInput::Text,
                    Some(FieldValue::Text("public".into())),
                ),
                field("username", "Username", FieldInput::Text, None),
                field("password", "Password", FieldInput::Secret, None),
                field(
                    "tls",
                    "TLS",
                    FieldInput::Choice {
                        choices: vec!["verify_full".into(), "require".into(), "disable".into()],
                    },
                    Some(FieldValue::Text("verify_full".into())),
                ),
            ],
            2,
        ),
        _ => return Err(ds_failure(DsErrorCode::ConfigIncompatible)),
    };
    Ok(ConnectorDescriptor {
        schema_version: 1,
        adapter_id: adapter
            .to_owned()
            .try_into()
            .map_err(|_| ds_failure(DsErrorCode::ConfigIncompatible))?,
        adapter_version: "1".to_owned().try_into().expect("static version"),
        config_version: 1,
        contract_version: 1,
        display_name: name.into(),
        support: ConnectorSupport::Registered,
        fields,
        capability: CapabilityDescriptor {
            // Descriptor source identifies the adapter, not a grant to any physical target.
            // Validation substitutes the exact revision SourceId before hashing capabilities.
            source_id: SourceId::new(adapter),
            dialect: adapter.into(),
            catalog_reader: true,
            preflight_reader: true,
            sql_query_executor: true,
            freshness_reader: true,
            supports_explain: adapter == "postgres",
            supports_read_only_tx: adapter == "postgres",
            read_only_mechanism: Some(if adapter == "postgres" {
                ReadOnlyMechanism::TransactionReadOnly
            } else {
                ReadOnlyMechanism::FileReadOnly
            }),
            max_concurrency: connections,
        },
        max_connections: NonZeroU64::new(connections as u64).expect("nonzero static limit"),
        release_evidence: None,
    })
}

fn field(id: &str, label: &str, input: FieldInput, default: Option<FieldValue>) -> DatasourceField {
    DatasourceField {
        id: FieldId::new(id).expect("static field"),
        label: label.into(),
        required: true,
        input,
        default,
    }
}

pub struct SqliteConnectorFactory;

#[async_trait]
impl ConnectorFactory for SqliteConnectorFactory {
    fn validate_config(&self, revision: &DatasourceRevision) -> Vec<FieldIssue> {
        let input = revision.input();
        let descriptor = builtin_descriptor("sqlite").expect("builtin descriptor");
        let mut issues = validate_datasource_fields(
            &descriptor.fields,
            &input.fields,
            input.credential.is_some(),
            true,
        );
        let path_field = FieldId::new("database_path").expect("static field");
        if input.adapter_id != descriptor.adapter_id
            || input.adapter_version != descriptor.adapter_version
            || input.config_version != descriptor.config_version
            || input.credential.is_some()
            || !matches!(&input.context, DatabaseContext::File { canonical_path } if input.fields.get(&path_field) == Some(&FieldValue::Text(canonical_path.to_string_lossy().into_owned())))
        {
            issues.push(FieldIssue {
                field: path_field,
                code: FieldIssueCode::Invalid,
            });
        }
        issues
    }

    async fn open(&self, input: ConnectorOpenInput) -> DsResult<Arc<dyn ManagedConnector>> {
        if !self.validate_config(&input.revision).is_empty() {
            return Err(ds_failure(DsErrorCode::InvalidField));
        }
        Ok(Arc::new(SqliteConnector::open_managed(input).await?))
    }
}
