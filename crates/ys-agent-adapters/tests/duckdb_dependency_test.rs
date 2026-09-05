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

#[test]
fn duckdb_security_configuration_is_observable() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("security.duckdb");
    drop(duckdb::Connection::open(&path).unwrap());
    let config = duckdb::Config::default()
        .access_mode(duckdb::AccessMode::ReadOnly)
        .unwrap()
        .enable_autoload_extension(false)
        .unwrap()
        .threads(1)
        .unwrap()
        .max_memory("256MB")
        .unwrap();
    let connection = duckdb::Connection::open_with_flags(path, config).unwrap();
    connection
        .execute_batch(
            "SET temp_directory=''; SET max_temp_directory_size='0B'; SET enable_external_access=false; SET autoload_known_extensions=false; SET autoinstall_known_extensions=false; SET lock_configuration=true;",
        )
        .unwrap();
    for (name, expected) in [
        ("access_mode", "read_only"),
        ("enable_external_access", "false"),
        ("autoload_known_extensions", "false"),
        ("autoinstall_known_extensions", "false"),
        ("lock_configuration", "true"),
        ("temp_directory", ""),
        ("max_temp_directory_size", "0 bytes"),
        ("threads", "1"),
    ] {
        let sql = format!("SELECT current_setting('{name}')::VARCHAR");
        let value: String = connection.query_row(&sql, [], |row| row.get(0)).unwrap();
        assert_eq!(value, expected, "{name}");
    }
}
