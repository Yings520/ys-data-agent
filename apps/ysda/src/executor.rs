use std::path::Path;

use rusqlite::types::ValueRef;

use crate::domain::{CellValue, QueryResult};
use crate::error::{AppError, AppResult};
use crate::sqlite::open_read_only;

pub struct SqliteExecutor;

impl SqliteExecutor {
    pub fn execute(path: &Path, sql: &str, max_rows: usize) -> AppResult<QueryResult> {
        let connection = open_read_only(path)?;
        let mut statement = connection.prepare(sql).map_err(AppError::SqlExecution)?;
        let columns = statement
            .column_names()
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let column_count = columns.len();
        let mut cursor = statement.query([]).map_err(AppError::SqlExecution)?;
        let mut result_rows = Vec::new();
        let mut truncated = false;

        while let Some(row) = cursor.next().map_err(AppError::SqlExecution)? {
            if result_rows.len() == max_rows {
                truncated = true;
                break;
            }

            let mut values = Vec::with_capacity(column_count);
            for index in 0..column_count {
                let value = row.get_ref(index).map_err(AppError::SqlExecution)?;
                values.push(to_owned_value(value));
            }
            result_rows.push(values);
        }

        Ok(QueryResult {
            columns,
            row_count: result_rows.len(),
            rows: result_rows,
            truncated,
        })
    }
}

fn to_owned_value(value: ValueRef<'_>) -> CellValue {
    match value {
        ValueRef::Null => CellValue::Null,
        ValueRef::Integer(value) => CellValue::Integer(value),
        ValueRef::Real(value) => CellValue::Real(value),
        ValueRef::Text(bytes) => CellValue::Text(String::from_utf8_lossy(bytes).into_owned()),
        ValueRef::Blob(bytes) => CellValue::Blob(format!("<{} bytes>", bytes.len())),
    }
}
