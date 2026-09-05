#[test]
fn embedded_engine_and_adapter_versions_are_real_and_publishable() {
    let sqlite_engine = rusqlite::version();
    let duckdb = duckdb::Connection::open_in_memory().expect("bundled DuckDB opens");
    let duckdb_engine: String = duckdb
        .query_row("SELECT version()", [], |row| row.get(0))
        .expect("DuckDB reports its engine version");

    assert!(!sqlite_engine.trim().is_empty());
    assert!(!duckdb_engine.trim().is_empty());
    println!("datasource.release.sqlite.engine={sqlite_engine} adapter=1");
    println!("datasource.release.duckdb.engine={duckdb_engine} adapter=1");
}
