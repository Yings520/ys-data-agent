#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use tempfile::TempDir;
use ys_agent_adapters::ResultPolicy;

pub struct TestDatabase {
    pub directory: TempDir,
    pub path: PathBuf,
}

pub fn create_test_database() -> TestDatabase {
    let directory = tempfile::tempdir().expect("temporary directory should be created");
    let path = directory.path().join("demo.db");
    let connection = Connection::open(&path).expect("test database should open");
    connection
        .execute_batch(
            "
            CREATE TABLE mart_orders (
                order_id INTEGER PRIMARY KEY,
                paid_amount REAL NOT NULL,
                paid_at TEXT NOT NULL,
                customer_email TEXT NOT NULL,
                country TEXT NOT NULL,
                channel TEXT NOT NULL
            );
            INSERT INTO mart_orders (
                order_id,
                paid_amount,
                paid_at,
                customer_email,
                country,
                channel
            ) VALUES
                (1, 120.5, '2026-08-13T10:00:00Z', 'alice@example.com', 'SG', 'web'),
                (2, 80.0,  '2026-08-14T10:00:00Z', 'bob@example.com', 'SG', 'store');
            ",
        )
        .expect("test schema and rows should be created");
    drop(connection);

    TestDatabase { directory, path }
}

pub fn test_policy() -> ResultPolicy {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("fixtures/policy/query-policy.json");
    let bytes = fs::read(path).expect("read test policy");
    ResultPolicy::from_json_bytes(&bytes).expect("valid test policy")
}
