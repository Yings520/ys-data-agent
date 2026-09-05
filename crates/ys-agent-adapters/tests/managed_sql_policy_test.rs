use std::collections::BTreeMap;

use ys_agent_adapters::{SqlPolicyDisposition, SqlReadOnlyPolicy, SupportedDialect};
use ys_agent_core::{AllowedDataScope, WorkspaceId};

#[test]
fn sqlite_rejects_quoted_nested_and_dynamic_file_access_before_open() {
    let policy = SqlReadOnlyPolicy::new(SupportedDialect::SQLite, 16_384);
    let scope = AllowedDataScope {
        workspace_id: WorkspaceId::new(),
        source_id: "explicit_test_scope".into(),
        relations: BTreeMap::new(),
    };
    for sql in [
        "SELECT \"load_extension\"('secret')",
        "SELECT coalesce(\"readfile\"('secret'), 'fallback')",
        "SELECT * FROM readfile('secret')",
        "ATTACH DATABASE 'secret' AS other",
        "SELECT 1; SELECT 2",
        "PRAGMA query_only=off",
        "VACUUM INTO 'secret'",
    ] {
        assert_eq!(
            policy.evaluate(sql, &scope).disposition,
            SqlPolicyDisposition::Rejected,
            "{sql}"
        );
    }
    assert_eq!(
        policy.evaluate("SELECT abs(-42)", &scope).disposition,
        SqlPolicyDisposition::Allowed
    );
}
