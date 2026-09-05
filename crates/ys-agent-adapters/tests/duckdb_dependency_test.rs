//! Native-link smoke gate; the managed DuckDB connector is integrated in task 5.
#[test]
fn pinned_duckdb_dependency_links_and_executes_on_this_platform() {
    let connection = duckdb::Connection::open_in_memory().expect("bundled DuckDB opens");
    let (version, answer): (String, i64) = connection
        .query_row("SELECT version(), 42::BIGINT", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .expect("native engine executes");
    assert!(version.starts_with("v1."), "unexpected engine: {version}");
    assert_eq!(answer, 42);
    println!(
        "duckdb adapter crate=1.10505.0 engine={version} platform={}-{}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
}
