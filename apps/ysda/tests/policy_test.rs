use ysda::policy::SqlPolicy;

#[test]
fn allows_single_select_and_read_only_cte() {
    assert!(
        SqlPolicy::evaluate("SELECT id FROM customers")
            .expect("SELECT should parse")
            .allowed
    );
    assert!(SqlPolicy::evaluate(
        "WITH totals AS (SELECT customer_id, SUM(amount) AS total FROM orders GROUP BY customer_id) SELECT * FROM totals"
    )
    .expect("CTE should parse")
    .allowed);
}

#[test]
fn rejects_mutation_ddl_pragma_and_multiple_statements() {
    for sql in [
        "DELETE FROM customers",
        "UPDATE customers SET name = 'Mallory'",
        "DROP TABLE customers",
        "PRAGMA writable_schema = 1",
        "SELECT 1; SELECT 2",
    ] {
        let decision = SqlPolicy::evaluate(sql).expect("test SQL should parse");
        assert!(!decision.allowed, "expected rejection for: {sql}");
        assert!(!decision.reasons.is_empty());
    }
}

#[test]
fn reports_invalid_sql_as_parse_error() {
    let error = SqlPolicy::evaluate("SELEC FROM").expect_err("invalid SQL must fail parsing");
    assert_eq!(error.category(), "SqlParseError");
}
