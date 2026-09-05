//! Shared real-driver acceptance entry. Callers supply actual database fixtures and trusted scope.
use ys_agent_core::{CellValue, ManagedConnector, QueryPreflightDecision, QueryRequest};

pub async fn assert_contract(
    connector: &dyn ManagedConnector,
    request: QueryRequest,
    relation: &str,
    time_column: &str,
) {
    let source = request.source_id.clone();
    assert!(connector.probe().await.unwrap().passed());
    assert!(
        !connector
            .observe_schema(&source)
            .await
            .unwrap()
            .relations
            .is_empty()
    );
    assert_eq!(
        connector.preflight(&request).await.unwrap().decision,
        QueryPreflightDecision::Allowed
    );
    let result = connector.execute_query(request.clone()).await.unwrap();
    assert_eq!(result.rows[0][0], CellValue::Integer(42));
    assert!(
        connector
            .read_freshness(&source, relation, time_column)
            .await
            .unwrap()
            .data_as_of
            .is_some()
    );
    connector.close().await.unwrap();
    connector.close().await.unwrap();
    assert!(connector.execute_query(request).await.is_err());
    assert!(connector.probe().await.is_err());
}
