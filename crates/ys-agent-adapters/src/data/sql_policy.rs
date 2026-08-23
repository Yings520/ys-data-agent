use std::collections::BTreeSet;
use std::ops::ControlFlow;

use sqlparser::ast::{Expr, ObjectName, Select, SelectItem, Statement, Visit, Visitor};
use sqlparser::dialect::{PostgreSqlDialect, SQLiteDialect};
use sqlparser::parser::Parser;
use ys_agent_core::{AllowedDataScope, ColumnPolicy, CoreError, CoreResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportedDialect {
    SQLite,
    Postgres,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqlPolicyDisposition {
    Allowed,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlPolicyReason {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlPolicyDecision {
    pub disposition: SqlPolicyDisposition,
    pub reasons: Vec<SqlPolicyReason>,
    pub referenced_relations: Vec<String>,
    pub referenced_columns: Vec<String>,
    pub selects_wildcard: bool,
}

impl SqlPolicyDecision {
    pub fn ensure_allowed(&self) -> CoreResult<()> {
        if self.disposition == SqlPolicyDisposition::Allowed {
            return Ok(());
        }
        let reason = self.reasons.first().cloned().unwrap_or(SqlPolicyReason {
            code: "sql_policy_rejected".to_owned(),
            message: "SQL policy rejected the query".to_owned(),
        });
        Err(CoreError::validation(
            stable_error_code(&reason.code),
            reason.message,
        ))
    }

    fn allowed(facts: AstFacts) -> Self {
        Self {
            disposition: SqlPolicyDisposition::Allowed,
            reasons: Vec::new(),
            referenced_relations: facts.relations.into_iter().collect(),
            referenced_columns: facts.columns.into_iter().collect(),
            selects_wildcard: facts.selects_wildcard,
        }
    }

    fn rejected(code: &str, message: impl Into<String>) -> Self {
        Self {
            disposition: SqlPolicyDisposition::Rejected,
            reasons: vec![SqlPolicyReason {
                code: code.to_owned(),
                message: message.into(),
            }],
            referenced_relations: Vec::new(),
            referenced_columns: Vec::new(),
            selects_wildcard: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SqlReadOnlyPolicy {
    dialect: SupportedDialect,
    max_sql_bytes: usize,
    blocked_functions: BTreeSet<String>,
}

impl SqlReadOnlyPolicy {
    pub fn new(dialect: SupportedDialect, max_sql_bytes: usize) -> Self {
        let blocked_functions = match dialect {
            SupportedDialect::SQLite => [
                "changes",
                "fts3_tokenizer",
                "last_insert_rowid",
                "load_extension",
                "random",
                "randomblob",
                "readfile",
                "writefile",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            SupportedDialect::Postgres => [
                "dblink",
                "lo_export",
                "lo_import",
                "nextval",
                "pg_advisory_lock",
                "pg_advisory_xact_lock",
                "pg_ls_dir",
                "pg_read_binary_file",
                "pg_read_file",
                "pg_sleep",
                "random",
                "set_config",
                "setval",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        };
        Self {
            dialect,
            max_sql_bytes,
            blocked_functions,
        }
    }

    pub fn with_additional_blocked_functions(
        mut self,
        names: impl IntoIterator<Item = String>,
    ) -> Self {
        self.blocked_functions
            .extend(names.into_iter().map(|name| name.to_ascii_lowercase()));
        self
    }

    pub fn evaluate(&self, sql: &str, scope: &AllowedDataScope) -> SqlPolicyDecision {
        if sql.len() > self.max_sql_bytes {
            return SqlPolicyDecision::rejected(
                "sql_too_large",
                format!(
                    "SQL is {} bytes; maximum is {}",
                    sql.len(),
                    self.max_sql_bytes
                ),
            );
        }

        let parsed = match self.dialect {
            SupportedDialect::SQLite => Parser::parse_sql(&SQLiteDialect {}, sql),
            SupportedDialect::Postgres => Parser::parse_sql(&PostgreSqlDialect {}, sql),
        };

        let statements = match parsed {
            Ok(statements) => statements,
            Err(error) => {
                return SqlPolicyDecision::rejected(
                    "sql_parse_error",
                    format!("SQL could not be parsed: {error}"),
                );
            }
        };

        if statements.len() != 1 {
            return SqlPolicyDecision::rejected(
                "statement_count_invalid",
                format!(
                    "expected exactly one statement, received {}",
                    statements.len()
                ),
            );
        }
        let Statement::Query(query) = &statements[0] else {
            return SqlPolicyDecision::rejected(
                "statement_not_read_only",
                "only a query statement is allowed",
            );
        };
        if !query.locks.is_empty() {
            return SqlPolicyDecision::rejected(
                "locking_query_rejected",
                "FOR UPDATE and other locking clauses are not allowed",
            );
        }

        let mut facts = AstFacts::default();
        let _ = statements.visit(&mut facts);

        if facts.contains_non_query_statement {
            return SqlPolicyDecision::rejected(
                "mutating_subquery_rejected",
                "a query may not contain a mutating nested statement",
            );
        }

        if facts.has_select_into {
            return SqlPolicyDecision::rejected(
                "select_into_rejected",
                "SELECT INTO creates data and is not allowed",
            );
        }

        if let Some(function) = facts
            .functions
            .iter()
            .find(|name| self.blocked_functions.contains(*name))
        {
            return SqlPolicyDecision::rejected(
                "function_not_allowed",
                format!("function {function} is blocked by query policy"),
            );
        }

        if let Some(decision) = validate_scope(&facts, scope) {
            return decision;
        }
        SqlPolicyDecision::allowed(facts)
    }
}

#[derive(Debug, Default)]
struct AstFacts {
    relations: BTreeSet<String>,
    columns: BTreeSet<String>,
    functions: BTreeSet<String>,
    selects_wildcard: bool,
    has_select_into: bool,
    has_dynamic_relation: bool,
    contains_non_query_statement: bool,
}

impl Visitor for AstFacts {
    type Break = ();

    fn pre_visit_statement(&mut self, statement: &Statement) -> ControlFlow<Self::Break> {
        if !matches!(statement, Statement::Query(_)) {
            self.contains_non_query_statement = true;
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_relation(&mut self, relation: &ObjectName) -> ControlFlow<Self::Break> {
        match normalized_object_name(relation) {
            Some(name) => {
                self.relations.insert(name);
            }
            None => self.has_dynamic_relation = true,
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_select(&mut self, select: &Select) -> ControlFlow<Self::Break> {
        self.has_select_into |= select.into.is_some();
        self.selects_wildcard |= select.projection.iter().any(|item| {
            matches!(
                item,
                SelectItem::Wildcard(_) | SelectItem::QualifiedWildcard(_, _)
            )
        });
        ControlFlow::Continue(())
    }

    fn pre_visit_expr(&mut self, expression: &Expr) -> ControlFlow<Self::Break> {
        match expression {
            Expr::Identifier(identifier) => {
                self.columns.insert(identifier.value.to_ascii_lowercase());
            }
            Expr::CompoundIdentifier(identifiers) => {
                if let Some(identifier) = identifiers.last() {
                    self.columns.insert(identifier.value.to_ascii_lowercase());
                }
            }
            Expr::Function(function) => {
                self.functions
                    .insert(function.name.to_string().to_ascii_lowercase());
            }
            _ => {}
        }
        ControlFlow::Continue(())
    }
}

fn normalized_object_name(name: &ObjectName) -> Option<String> {
    name.0
        .iter()
        .map(|part| {
            part.as_ident()
                .map(|identifier| identifier.value.to_ascii_lowercase())
        })
        .collect::<Option<Vec<_>>>()
        .map(|parts| parts.join("."))
}

fn validate_scope(facts: &AstFacts, scope: &AllowedDataScope) -> Option<SqlPolicyDecision> {
    let mut scoped_relations = Vec::new();
    for relation in &facts.relations {
        let Some(columns) = find_scope_relation(scope, relation) else {
            return Some(SqlPolicyDecision::rejected(
                "relation_not_allowed",
                format!("relation {relation} is outside the allowed scope"),
            ));
        };
        scoped_relations.push(columns);
    }

    for column in &facts.columns {
        let matching_policies = scoped_relations
            .iter()
            .filter_map(|columns| columns.get(column))
            .copied()
            .collect::<Vec<_>>();
        if matching_policies.is_empty() {
            return Some(SqlPolicyDecision::rejected(
                "column_not_allowed",
                format!("column {column} is outside the allowed scope"),
            ));
        }
        if matching_policies
            .iter()
            .all(|policy| *policy == ColumnPolicy::Deny)
        {
            return Some(SqlPolicyDecision::rejected(
                "column_denied",
                format!("column {column} is denied"),
            ));
        }
    }

    if facts.selects_wildcard
        && scoped_relations
            .iter()
            .any(|columns| columns.values().any(|policy| *policy == ColumnPolicy::Deny))
    {
        return Some(SqlPolicyDecision::rejected(
            "wildcard_includes_denied_column",
            "a wildcard would read a denied column",
        ));
    }
    None
}

fn find_scope_relation<'a>(
    scope: &'a AllowedDataScope,
    requested: &str,
) -> Option<&'a std::collections::BTreeMap<String, ColumnPolicy>> {
    if let Some(columns) = scope.relations.get(requested) {
        return Some(columns);
    }

    let mut matches = scope.relations.iter().filter(|(configured, _)| {
        configured
            .rsplit_once('.')
            .is_some_and(|(_, suffix)| suffix == requested)
    });
    let (_, columns) = matches.next()?;
    matches.next().is_none().then_some(columns)
}

fn stable_error_code(code: &str) -> &'static str {
    match code {
        "sql_too_large" => "sql_too_large",
        "sql_parse_error" => "sql_parse_error",
        "statement_count_invalid" => "statement_count_invalid",
        "statement_not_read_only" => "statement_not_read_only",
        "locking_query_rejected" => "locking_query_rejected",
        "mutating_subquery_rejected" => "mutating_subquery_rejected",
        "select_into_rejected" => "select_into_rejected",
        "dynamic_relation_rejected" => "dynamic_relation_rejected",
        "function_not_allowed" => "function_not_allowed",
        "relation_not_allowed" => "relation_not_allowed",
        "column_not_allowed" => "column_not_allowed",
        "column_denied" => "column_denied",
        "wildcard_includes_denied_column" => "wildcard_includes_denied_column",
        _ => "sql_policy_rejected",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ys_agent_core::{AllowedDataScope, ColumnPolicy, WorkspaceId};

    use super::{SqlPolicyDisposition, SqlReadOnlyPolicy, SupportedDialect};

    fn scope() -> AllowedDataScope {
        AllowedDataScope {
            workspace_id: WorkspaceId::new(),
            source_id: "test".to_owned(),
            relations: [(
                "mart_orders".to_owned(),
                [
                    ("order_id".to_owned(), ColumnPolicy::Allow),
                    ("customer_email".to_owned(), ColumnPolicy::Redact),
                ]
                .into_iter()
                .collect::<BTreeMap<_, _>>(),
            )]
            .into_iter()
            .collect(),
        }
    }

    #[test]
    fn comments_do_not_confuse_the_ast_policy() {
        let policy = SqlReadOnlyPolicy::new(SupportedDialect::SQLite, 1024);
        let decision = policy.evaluate(
            "SELECT order_id FROM mart_orders -- DELETE is only a comment",
            &scope(),
        );
        assert_eq!(decision.disposition, SqlPolicyDisposition::Allowed);
    }

    #[test]
    fn postgres_select_into_is_rejected() {
        let policy = SqlReadOnlyPolicy::new(SupportedDialect::Postgres, 1024);
        let decision = policy.evaluate(
            "SELECT order_id INTO copied_orders FROM mart_orders",
            &scope(),
        );
        assert_eq!(decision.reasons[0].code, "select_into_rejected");
    }

    #[test]
    fn blocked_function_is_rejected() {
        let policy = SqlReadOnlyPolicy::new(SupportedDialect::Postgres, 1024);
        let decision = policy.evaluate(
            "SELECT pg_read_file(customer_email) FROM mart_orders",
            &scope(),
        );
        assert_eq!(decision.reasons[0].code, "function_not_allowed");
    }
}
