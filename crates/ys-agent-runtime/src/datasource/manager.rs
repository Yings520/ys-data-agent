use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use tokio::sync::Mutex;
use ys_agent_core::*;

use super::SourcePolicy;

pub struct ConnectorManager {
    repository: Arc<dyn DatasourceRepository>,
    vault: Arc<dyn DatasourceVault>,
    catalog: Arc<dyn ConnectorCatalog>,
    policy: Arc<SourcePolicy>,
    state: Mutex<ManagerState>,
}

#[derive(Default)]
struct ManagerState {
    closed: bool,
    connectors: HashMap<String, CacheEntry>,
    runs: HashMap<RunId, String>,
}

#[derive(Clone)]
struct CacheEntry {
    connector: Arc<dyn ManagedConnector>,
    governance: DatasourceGovernanceContext,
    capability: CapabilityDescriptor,
}

impl ConnectorManager {
    pub fn new(
        repository: Arc<dyn DatasourceRepository>,
        vault: Arc<dyn DatasourceVault>,
        catalog: Arc<dyn ConnectorCatalog>,
        policy: Arc<SourcePolicy>,
    ) -> Self {
        Self {
            repository,
            vault,
            catalog,
            policy,
            state: Mutex::new(ManagerState::default()),
        }
    }

    fn descriptor(&self, binding: &RunDatasourceBinding) -> DsResult<ConnectorDescriptor> {
        self.catalog
            .descriptors()?
            .into_iter()
            .find(|descriptor| {
                &descriptor.adapter_id == binding.adapter_id()
                    && &descriptor.adapter_version == binding.adapter_version()
            })
            .ok_or_else(|| error(DsErrorCode::ConfigIncompatible))
    }

    async fn inputs(
        &self,
        binding: &RunDatasourceBinding,
    ) -> DsResult<(
        DatasourceRevision,
        DatasourceGovernanceContext,
        DatasourceValidationInputs,
        ConnectorDescriptor,
    )> {
        binding
            .validate_supported()
            .map_err(|_| error(DsErrorCode::ConfigIncompatible))?;
        let detail = self.repository.load_revision(binding.revision()).await?;
        let governance = self.policy.governance_for(&detail.revision)?;
        let descriptor = self.descriptor(binding)?;
        let mut capability = descriptor.capability.clone();
        capability.source_id = binding.source_id().clone();
        capability.max_concurrency = capability
            .max_concurrency
            .min(governance.budget.max_concurrency);
        let inputs = DatasourceValidationInputs::new(
            &detail.revision,
            &capability,
            governance.policy_digest.clone(),
        )
        .map_err(|_| error(DsErrorCode::ValidationStale))?;
        if !binding.evidence().matches(&inputs)
            || detail.validation.as_ref() != Some(binding.evidence())
        {
            return Err(error(DsErrorCode::ValidationStale));
        }
        Ok((detail.revision, governance, inputs, descriptor))
    }

    fn context(
        binding: RunDatasourceBinding,
        governance: &DatasourceGovernanceContext,
        capability: &CapabilityDescriptor,
    ) -> DsResult<RunDatasourceContext> {
        let mut tools = Vec::new();
        if capability.catalog_reader {
            tools.push("inspect_schema".into());
        }
        if capability.preflight_reader {
            tools.push("query_preflight".into());
        }
        if capability.sql_query_executor {
            tools.push("query_data".into());
        }
        if capability.freshness_reader {
            tools.push("read_freshness".into());
        }
        let context_namespace = DatasourceDigest::of(&(
            binding
                .digest()
                .map_err(|_| error(DsErrorCode::ValidationStale))?,
            governance.policy_digest.clone(),
        ))
        .map_err(|_| error(DsErrorCode::ValidationStale))?;
        Ok(RunDatasourceContext {
            schema_version: 1,
            binding,
            data_scope: governance.data_scope.clone(),
            result_policy: governance.result_policy.clone(),
            query_budget: governance.budget.clone(),
            tools,
            context_namespace,
        })
    }
}

#[async_trait]
impl RunDatasourceResolver for ConnectorManager {
    async fn resolve(&self, run_id: RunId) -> DsResult<ResolvedRunDatasource> {
        let binding = self.repository.load_run_binding(run_id).await?;
        if binding.run_id() != run_id {
            return Err(error(DsErrorCode::Conflict));
        }
        let key = DatasourceDigest::of(binding.evidence())
            .map_err(|_| error(DsErrorCode::ValidationStale))?
            .hex();
        {
            let state = self.state.lock().await;
            if state.closed {
                return Err(error(DsErrorCode::Cancelled));
            }
            if let Some(existing_key) = state.runs.get(&run_id) {
                if existing_key != &key {
                    return Err(error(DsErrorCode::Conflict));
                }
                let entry = state
                    .connectors
                    .get(&key)
                    .ok_or_else(|| error(DsErrorCode::Storage))?;
                return Ok(ResolvedRunDatasource {
                    context: Self::context(binding, &entry.governance, &entry.capability)?,
                    connector: entry.connector.clone(),
                });
            }
        }
        let (revision, governance, inputs, descriptor) = self.inputs(&binding).await?;
        let capability = inputs.capability().clone();
        let context = Self::context(binding.clone(), &governance, &capability)?;

        // The lock intentionally spans Factory::open: concurrent requests for an identical full
        // binding identity merge into one creation, and no half-open handle becomes visible.
        let mut state = self.state.lock().await;
        if state.closed {
            return Err(error(DsErrorCode::Cancelled));
        }
        if let Some(existing_key) = state.runs.get(&run_id) {
            if existing_key != &key {
                return Err(error(DsErrorCode::Conflict));
            }
            let entry = state
                .connectors
                .get(&key)
                .ok_or_else(|| error(DsErrorCode::Storage))?;
            return Ok(ResolvedRunDatasource {
                context,
                connector: entry.connector.clone(),
            });
        }
        if let Some(entry) = state.connectors.get(&key).cloned() {
            state.runs.insert(run_id, key);
            return Ok(ResolvedRunDatasource {
                context,
                connector: entry.connector,
            });
        }

        let secret = match revision.input().credential {
            Some(reference) => Some(self.vault.read(reference).await?),
            None => None,
        };
        let factory = self
            .catalog
            .factory(&descriptor.adapter_id, &descriptor.adapter_version)?;
        if !factory.validate_config(&revision).is_empty() {
            return Err(error(DsErrorCode::InvalidField));
        }
        let connector = factory
            .open(ConnectorOpenInput {
                revision,
                secret,
                governance: governance.clone(),
            })
            .await?;
        if let Err(error) = connector.probe().await {
            let _ = connector.close().await;
            return Err(error);
        }
        state.connectors.insert(
            key.clone(),
            CacheEntry {
                connector: connector.clone(),
                governance,
                capability,
            },
        );
        state.runs.insert(run_id, key);
        Ok(ResolvedRunDatasource { context, connector })
    }

    async fn release(&self, run_id: RunId) -> DsResult<()> {
        let connector = {
            let mut state = self.state.lock().await;
            let Some(key) = state.runs.remove(&run_id) else {
                return Ok(());
            };
            if state.runs.values().any(|active| active == &key) {
                None
            } else {
                state.connectors.remove(&key).map(|entry| entry.connector)
            }
        };
        if let Some(connector) = connector {
            connector.close().await?;
        }
        Ok(())
    }

    async fn close(&self) -> DsResult<()> {
        let connectors = {
            let mut state = self.state.lock().await;
            if state.closed {
                return Ok(());
            }
            state.closed = true;
            state.runs.clear();
            state
                .connectors
                .drain()
                .map(|(_, entry)| entry.connector)
                .collect::<Vec<_>>()
        };
        let mut first_error = None;
        for connector in connectors {
            if let Err(error) = connector.close().await
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

fn error(code: DsErrorCode) -> DsError {
    DsError {
        code,
        field: None,
        remediation: match code {
            DsErrorCode::ValidationStale => DsRemediation::Revalidate,
            DsErrorCode::Conflict => DsRemediation::Refresh,
            _ => DsRemediation::Retry,
        },
        operation_id: None,
    }
}
