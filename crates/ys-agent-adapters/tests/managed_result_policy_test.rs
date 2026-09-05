use ys_agent_adapters::{ColumnAction, ResultPolicy};
use ys_agent_core::SourceId;

#[test]
fn ambiguous_cross_relation_column_uses_strictest_policy() {
    let policy = ResultPolicy::from_json_bytes(
        br#"{
        "schema_version": 1, "allowed_sources": {"test": {"relations": {
            "public_values": {"columns": {"value": "allow"}},
            "private_values": {"columns": {"value": "redact"}}
        }}}
    }"#,
    )
    .unwrap();
    assert_eq!(
        policy.action(
            &SourceId::new("test"),
            &["public_values".into(), "private_values".into()],
            "value"
        ),
        Some(ColumnAction::Redact)
    );
}
