use sqlparser::ast::Statement;
use sqlparser::dialect::SQLiteDialect;
use sqlparser::parser::Parser;

use crate::domain::PolicyDecision;
use crate::error::{AppError, AppResult};
pub struct SqlPolicy;
impl SqlPolicy {
    pub fn evaluate(sql: &str) -> AppResult<PolicyDecision> {
        let dialect = SQLiteDialect {};
        let statements = Parser::parse_sql(&dialect, sql).map_err(AppError::SqlParse)?;
        if statements.len() != 1 {
            return Ok(PolicyDecision::deny(format!(
                "expected exactly one statement, received {}",
                statements.len()
            )));
        }

        match &statements[0] {
            Statement::Query(_) => Ok(PolicyDecision::allow()),
            statement => Ok(PolicyDecision::deny(format!(
                "only read-only query statements are allowed; received {statement}"
            ))),
        }
    }
}
