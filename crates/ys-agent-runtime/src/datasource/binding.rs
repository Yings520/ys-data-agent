use std::sync::Arc;

use async_trait::async_trait;
use ys_agent_core::{
    ConnectorCatalog, DatasourceRepository, DatasourceScope, DatasourceValidationInputs, DsError,
    DsErrorCode, DsRemediation, DsResult, RunDatasourceBinding, RunDatasourceBindingSource, RunId,
};

use super::SourcePolicy;

/// Creates a new immutable Run binding from the exact committed Session selection.
///
/// The selection and its evidence are checked again by the Store in the same transaction that
/// creates the Run, so a concurrent switch or invalidation cannot produce a mixed snapshot.
pub struct ActiveRunDatasourceBindingSource {
    repository: Arc<dyn DatasourceRepository>,
    catalog: Arc<dyn ConnectorCatalog>,
    policy: Arc<SourcePolicy>,
}

impl ActiveRunDatasourceBindingSource {
    pub fn new(
        repository: Arc<dyn DatasourceRepository>,
        catalog: Arc<dyn ConnectorCatalog>,
        policy: Arc<SourcePolicy>,
    ) -> Self {
        Self {
            repository,
            catalog,
            policy,
        }
    }
}

#[async_trait]
impl RunDatasourceBindingSource for ActiveRunDatasourceBindingSource {
    async fn bind_new_run(
        &self,
        run_id: RunId,
        scope: Option<DatasourceScope>,
        retry_of: Option<RunId>,
    ) -> DsResult<RunDatasourceBinding> {
        let scope = match (scope, retry_of) {
            (Some(scope), _) => scope,
            (None, Some(previous)) => self.repository.load_run_binding(previous).await?.scope(),
            (None, None) => return Err(error(DsErrorCode::ValidationStale)),
        };
        let snapshot = self.repository.load(scope).await?;
        let revision_id = snapshot
            .selection
            .current
            .ok_or_else(|| error(DsErrorCode::ValidationStale))?;
        let detail = self.repository.load_revision(revision_id).await?;
        let governance = self.policy.governance_for(&detail.revision)?;
        let input = detail.revision.input();
        let descriptor = self
            .catalog
            .descriptors()?
            .into_iter()
            .find(|descriptor| {
                descriptor.adapter_id == input.adapter_id
                    && descriptor.adapter_version == input.adapter_version
            })
            .ok_or_else(|| error(DsErrorCode::ConfigIncompatible))?;
        let source = input
            .source_id
            .as_ref()
            .ok_or_else(|| error(DsErrorCode::PolicyDenied))?;
        let mut capability = descriptor.capability;
        capability.source_id = source.clone();
        capability.max_concurrency = capability
            .max_concurrency
            .min(governance.budget.max_concurrency);
        let inputs = DatasourceValidationInputs::new(
            &detail.revision,
            &capability,
            governance.policy_digest,
        )
        .map_err(|_| error(DsErrorCode::ValidationStale))?;
        if !detail.is_ready(&inputs) {
            return Err(error(DsErrorCode::ValidationStale));
        }
        RunDatasourceBinding::from_validated(
            run_id,
            scope,
            snapshot.selection.selection_version,
            &detail.revision,
            detail
                .validation
                .as_ref()
                .ok_or_else(|| error(DsErrorCode::ValidationStale))?,
            &inputs,
        )
        .map_err(|_| error(DsErrorCode::ValidationStale))
    }
}

fn error(code: DsErrorCode) -> DsError {
    DsError {
        code,
        field: None,
        remediation: match code {
            DsErrorCode::PolicyDenied => DsRemediation::RepairPolicy,
            DsErrorCode::ConfigIncompatible => DsRemediation::UpgradeAdapter,
            _ => DsRemediation::Revalidate,
        },
        operation_id: None,
    }
}
