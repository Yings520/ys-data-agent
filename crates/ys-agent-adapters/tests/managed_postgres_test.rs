use std::collections::BTreeMap;

use ys_agent_adapters::data::PostgresConnectorFactory;
use ys_agent_core::*;

#[path = "support/connector_contract.rs"]
mod connector_contract;

fn open_input(password: &str, database: &str, tls: &str) -> ConnectorOpenInput {
    open_input_for(password, database, tls, "ysda_reader", 55432)
}

fn open_input_for(
    password: &str,
    database: &str,
    tls: &str,
    username: &str,
    port: i64,
) -> ConnectorOpenInput {
    let workspace_id = WorkspaceId::new();
    let profile_id = ProfileId::new();
    let reference = DatasourceSecretRef::new(workspace_id, profile_id, 1).unwrap();
    let source = SourceId::new("postgres_demo");
    let relations: BTreeMap<String, BTreeMap<String, ColumnPolicy>> = [(
        "public.orders".into(),
        [
            ("order_id".into(), ColumnPolicy::Allow),
            ("paid_amount".into(), ColumnPolicy::Allow),
            ("paid_at".into(), ColumnPolicy::Allow),
            ("customer_email".into(), ColumnPolicy::Redact),
        ]
        .into(),
    )]
    .into();
    let fields = [
        ("host", FieldValue::Text("127.0.0.1".into())),
        ("port", FieldValue::Integer(port)),
        ("database", FieldValue::Text(database.into())),
        ("schema", FieldValue::Text("public".into())),
        ("username", FieldValue::Text(username.into())),
        ("tls", FieldValue::Text(tls.into())),
    ]
    .into_iter()
    .map(|(name, value)| (FieldId::new(name).unwrap(), value))
    .collect();
    ConnectorOpenInput {
        revision: DatasourceRevision::new(DatasourceRevisionInput {
            schema_version: 1,
            workspace_id,
            profile_id,
            revision: 1,
            adapter_id: "postgres".try_into().unwrap(),
            adapter_version: "1".try_into().unwrap(),
            config_version: 1,
            source_id: Some(source),
            fields,
            context: DatabaseContext::Database {
                catalog: Some(format!("127.0.0.1:{port}")),
                database: database.into(),
                schema: "public".into(),
            },
            credential: Some(reference),
        })
        .unwrap(),
        secret: Some(SecretLease {
            reference,
            value: SecretValue::from_utf8(password.into()),
        }),
        governance: DatasourceGovernanceContext {
            data_scope: AllowedDataScope {
                workspace_id,
                source_id: "postgres_demo".into(),
                relations: relations.clone(),
            },
            result_policy: relations,
            budget: QueryBudget {
                max_concurrency: 2,
                statement_timeout_ms: 2_000,
                acquire_timeout_ms: 1_000,
                ..QueryBudget::default()
            },
            policy_digest: DatasourceDigest::of(&"explicit-postgres-test-authority").unwrap(),
            allowed_roots: vec![],
        },
    }
}

fn request(input: &ConnectorOpenInput, sql: &str) -> QueryRequest {
    QueryRequest {
        source_id: input.revision.input().source_id.clone().unwrap(),
        sql: sql.into(),
        parameters: vec![],
        budget: input.governance.budget.clone(),
        query_tag: "managed-postgres-contract".into(),
        scope: input.governance.data_scope.clone(),
        confirmation_granted: true,
    }
}

#[test]
fn structured_factory_rejects_socket_hosts_without_io() {
    let factory = PostgresConnectorFactory;
    let valid = open_input("unused", "ysda_test", "disable");
    assert!(factory.validate_config(&valid.revision).is_empty());

    let mut socket = open_input("unused", "ysda_test", "disable");
    let raw = socket.revision.input().clone();
    let mut fields = raw.fields;
    fields.insert(
        FieldId::new("host").unwrap(),
        FieldValue::Text("/tmp".into()),
    );
    socket.revision = DatasourceRevision::new(DatasourceRevisionInput { fields, ..raw }).unwrap();
    assert!(!factory.validate_config(&socket.revision).is_empty());
}

#[tokio::test]
#[ignore = "requires fixtures/postgres/compose.yaml"]
async fn managed_postgres_real_contract_errors_timeout_and_close() {
    let factory = PostgresConnectorFactory;
    let input = open_input("ysda-reader-test", "ysda_test", "disable");
    let query = request(
        &input,
        "SELECT order_id FROM public.orders ORDER BY order_id",
    );
    let connector = factory.open(input).await.unwrap();
    connector_contract::assert_contract(
        connector.as_ref(),
        query,
        CellValue::Integer(1),
        "public.orders",
        "paid_at",
    )
    .await;

    let limited_input = open_input("ysda-reader-test", "ysda_test", "disable");
    let mut slow = request(
        &limited_input,
        "SELECT count(*) FROM public.orders a CROSS JOIN public.orders b CROSS JOIN public.orders c CROSS JOIN public.orders d CROSS JOIN public.orders e CROSS JOIN public.orders f CROSS JOIN public.orders g CROSS JOIN public.orders h CROSS JOIN public.orders i CROSS JOIN public.orders j CROSS JOIN public.orders k CROSS JOIN public.orders l CROSS JOIN public.orders m CROSS JOIN public.orders n CROSS JOIN public.orders o",
    );
    slow.budget.statement_timeout_ms = 10;
    let healthy = request(
        &limited_input,
        "SELECT order_id FROM public.orders ORDER BY order_id",
    );
    let limited = factory.open(limited_input).await.unwrap();
    let timeout = limited
        .execute_query(slow)
        .await
        .expect_err("server query is cancelled");
    assert_eq!(timeout.code(), "datasource_timeout");
    assert!(limited.execute_query(healthy.clone()).await.is_ok());
    let unsupported = QueryRequest {
        sql: "SELECT INTERVAL '1 day'".into(),
        ..healthy
    };
    assert_eq!(
        limited.execute_query(unsupported).await.unwrap_err().code(),
        "unsupported_postgres_type"
    );
    limited.close().await.unwrap();

    let excessive = factory
        .open(open_input_for(
            "ysda-test",
            "ysda_test",
            "disable",
            "ysda",
            55432,
        ))
        .await
        .unwrap();
    assert_eq!(
        excessive.probe().await.unwrap_err().code,
        DsErrorCode::PermissionDenied
    );
    excessive.close().await.unwrap();

    let bad_password = factory
        .open(open_input("wrong-canary", "ysda_test", "disable"))
        .await
        .err()
        .expect("wrong password fails");
    assert_eq!(bad_password.code, DsErrorCode::AuthenticationFailed);
    let bad_target = factory
        .open(open_input(
            "ysda-reader-test",
            "missing_database",
            "disable",
        ))
        .await
        .err()
        .expect("missing database fails");
    assert_eq!(bad_target.code, DsErrorCode::TargetMissing);
    let bad_tls = factory
        .open(open_input("ysda-reader-test", "ysda_test", "require"))
        .await
        .err()
        .expect("TLS mismatch fails");
    assert_eq!(bad_tls.code, DsErrorCode::Protocol);
    let network = factory
        .open(open_input_for(
            "unused",
            "ysda_test",
            "disable",
            "ysda_reader",
            1,
        ))
        .await
        .err()
        .expect("unreachable port fails");
    assert_eq!(network.code, DsErrorCode::Network);
    let displayed = format!("{bad_password} {bad_target} {bad_tls} {network}");
    assert!(!displayed.contains("wrong-canary"));
}
