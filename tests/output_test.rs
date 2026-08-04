use ysda::domain::{
    AgentRun, CellValue, ColumnSchema, GeneratedQuery, PolicyDecision, QueryResult, RunErrorRecord,
    RunEvent, SchemaSnapshot, TableSchema, UserQuestion,
};
use ysda::output::{render_result, render_run, render_schema};

#[test]

fn renders_tables_and_truncation_notice() {
    let result = QueryResult {
        columns: vec!["name".to_owned(), "amount".to_owned()],
        rows: vec![vec![
            CellValue::Text("Alice".to_owned()),
            CellValue::Real(120.5),
        ]],
        row_count: 1,
        truncated: true,
    };

    let rendered = render_result(&result);
    assert!(rendered.contains("Alice"));
    assert!(rendered.contains("120.5"));
    assert!(rendered.contains("(truncated)"));
}

#[test]
fn renders_schema_columns() {
    let schema = SchemaSnapshot {
        tables: vec![TableSchema {
            name: "customers".to_owned(),
            columns: vec![ColumnSchema {
                name: "id".to_owned(),
                data_type: "INTEGER".to_owned(),
                not_null: true,
                primary_key_position: 1,
            }],
        }],
    };

    let rendered = render_schema(&schema);

    assert!(rendered.contains("customers"));
    assert!(rendered.contains("id"));
    assert!(rendered.contains("INTEGER"));
}

#[test]
fn renders_populated_run_sections() {
    let mut run = AgentRun::new(UserQuestion::new("list customers"));
    run.generated_query = Some(GeneratedQuery {
        sql: "SELECT name FROM customers".to_owned(),
        explanation: "Lists customer names".to_owned(),
    });
    run.policy_decision = Some(PolicyDecision::deny("query is not read-only"));
    run.result = Some(QueryResult {
        columns: vec!["name".to_owned()],
        rows: vec![vec![CellValue::Text("Alice".to_owned())]],
        row_count: 1,
        truncated: false,
    });
    run.events.push(RunEvent {
        stage: "policy".to_owned(),
        elapsed_ms: 12,
        message: "rejected".to_owned(),
    });
    run.error = Some(RunErrorRecord {
        category: "UnsafeSqlError".to_owned(),
        message: "query is not read-only".to_owned(),
    });

    let rendered = render_run(&run);

    assert!(rendered.contains("Run ID:"));
    assert!(rendered.contains("SQL:\nSELECT name FROM customers"));
    assert!(rendered.contains("Policy: denied (query is not read-only)"));
    assert!(rendered.contains("Alice"));
    assert!(rendered.contains("- policy at 12 ms: rejected"));
    assert!(rendered.contains("Error:\nUnsafeSqlError\nquery is not read-only"));
}
