use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use crate::error::{AppError, AppResult};

pub fn open_read_only(path: &Path) -> AppResult<Connection> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|source| AppError::DatabaseConnection {
        path: path.to_path_buf(),
        source,
    })?;
    connection
        .pragma_update(None, "query_only", true)
        .map_err(AppError::SchemaInspection)?;
    Ok(connection)
}
