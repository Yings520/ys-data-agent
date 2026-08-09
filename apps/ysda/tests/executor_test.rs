mod support;

use ysda::executor::SqliteExecutor;
use ysda::sqlite::open_read_only;

#[test]
fn executes_query_and_maps_columns_and_values() {
    let database = support::create_test_database();

    let result = SqliteExecutor::execute(
        &database.path,
        "SELECT name, region FROM customers ORDER BY id",
        100,
    )
    .expect("query should execute");

    assert_eq!(result.columns, vec!["name", "region"]);
    assert_eq!(result.row_count, 2);
    assert_eq!(result.rows[0][0].to_string(), "Alice");
    assert!(!result.truncated);
}

#[test]
fn stops_after_limit_and_marks_result_truncated() {
    let database = support::create_test_database();

    let result = SqliteExecutor::execute(&database.path, "SELECT id FROM orders ORDER BY id", 2)
        .expect("query should execute");

    assert_eq!(result.row_count, 2);
    assert_eq!(result.rows.len(), 2);
    assert!(result.truncated);
}

#[test]
fn read_only_connection_rejects_mutation_as_second_defense() {
    let database = support::create_test_database();
    let connection = open_read_only(&database.path).expect("read-only connection should open");

    let error = connection
        .execute("DELETE FROM customers", [])
        .expect_err("read-only database must reject DELETE");

    assert!(error.to_string().to_lowercase().contains("readonly"));
}
