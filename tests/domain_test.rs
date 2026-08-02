use ysda::domain::{
    AgentRun, CellValue, ColumnSchema, GeneratedQuery, PolicyDecision, QueryResult, SchemaSnapshot,
    TableSchema, UserQuestion,
};

#[test]
fn trace_serialization_excludes_query_rows_values() {
    let mut run = AgentRun::new(UserQuestion::new("top customer"));
    run.schema = Some(SchemaSnapshot {
        tables: vec![TableSchema {
            name: "customers".to_owned(),
            columns: vec![ColumnSchema {
                name: "name".to_owned(),
                data_type: "TEXT".to_owned(),
                not_null: true,
                primary_key_position: 0,
            }],
        }],
    });
    run.generated_query = Some(GeneratedQuery {
        sql: "SELECT name FROM customers".to_owned(),
        explanation: "Read customer names".to_owned(),
    });
    run.policy_decision = Some(PolicyDecision::allow());
    run.result = Some(QueryResult {
        columns: vec!["name".to_owned()],
        rows: vec![vec![CellValue::Text("Alice".to_owned())]],
        row_count: 1,
        truncated: false,
    });
    let json = serde_json::to_string(&run).expect("AgentRun should serialize");
    assert!(json.contains("SELECT name FROM customers"));
    assert!(!json.contains("Alice"));
}
