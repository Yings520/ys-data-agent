use ysda::domain::{AgentRun, CellValue, QueryResult, UserQuestion};
use ysda::trace::TraceRecorder;

#[test]

fn saves_and_loads_trace_without_row_values() {
    let directory = tempfile::tempdir().expect("temporary directory should exist");
    let recorder = TraceRecorder::new(directory.path());
    let mut run = AgentRun::new(UserQuestion::new("show secret"));
    run.result = Some(QueryResult {
        columns: vec!["value".to_owned()],
        rows: vec![vec![CellValue::Text("secret-row-value".to_owned())]],
        row_count: 1,
        truncated: false,
    });
    let path = recorder.save(&run).expect("trace should save");
    let raw = std::fs::read_to_string(path).expect("trace file should be readable");
    let loaded = recorder.load(run.run_id).expect("trace should load");

    assert!(!raw.contains("secret-row-value"));
    assert_eq!(loaded.run_id, run.run_id);
    assert!(
        loaded
            .result
            .expect("result shape should remain")
            .rows
            .is_empty()
    );
}
