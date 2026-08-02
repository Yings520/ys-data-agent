#![allow(dead_code)]

use std::path::PathBuf;

use rusqlite::Connection;
use tempfile::TempDir;

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
            CREATE TABLE customers (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                region TEXT NOT NULL
            );
            CREATE TABLE orders (
                id INTEGER PRIMARY KEY,
                customer_id INTEGER NOT NULL,
                amount REAL NOT NULL,
                FOREIGN KEY (customer_id) REFERENCES customers(id)
            );
            INSERT INTO customers (id, name, region) VALUES
                (1, 'Alice', 'APAC'),
                (2, 'Bob', 'EMEA');
            INSERT INTO orders (id, customer_id, amount) VALUES
                (1, 1, 120.5),
                (2, 1, 80.0),
                (3, 2, 50.0);
            ",
        )
        .expect("test schema and rows should be created");
    drop(connection);

    TestDatabase { directory, path }
}
