use std::path::Path;

use crate::domain::{ColumnSchema, SchemaSnapshot, TableSchema};
use crate::error::{AppError, AppResult};
use crate::sqlite::open_read_only;

pub struct SqliteCatalog;

impl SqliteCatalog {
    pub fn inspect(path: &Path) -> AppResult<SchemaSnapshot> {
        let connection = open_read_only(path)?;
        let table_names = {
            let mut statement = connection
                .prepare(
                    "
                SELECT name
                FROM sqlite_schema
                WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
                ORDER BY name",
                )
                .map_err(AppError::SchemaInspection)?;

            statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(AppError::SchemaInspection)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(AppError::SchemaInspection)?
        };
        let mut tables = Vec::with_capacity(table_names.len());
        for table_name in table_names {
            let pragma = format!("PRAGMA table_info({})", quote_identifier(&table_name));
            let mut statement = connection
                .prepare(&pragma)
                .map_err(AppError::SchemaInspection)?;
            let columns = statement
                .query_map([], |row| {
                    Ok(ColumnSchema {
                        name: row.get(1)?,
                        data_type: row.get(2)?,
                        not_null: row.get::<_, i64>(3)? != 0,
                        primary_key_position: row.get::<_, i64>(5)? as u32,
                    })
                })
                .map_err(AppError::SchemaInspection)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(AppError::SchemaInspection)?;

            tables.push(TableSchema {
                name: table_name,
                columns,
            });
        }
        Ok(SchemaSnapshot { tables })
    }
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::quote_identifier;

    #[test]
    fn quotes_embedded_double_quotes() {
        assert_eq!(quote_identifier("odd\"name"), "\"odd\"\"name\"");
    }
}
