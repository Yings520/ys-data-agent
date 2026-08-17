CREATE TABLE sessions (
    session_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    principal_id TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE tasks (
    task_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    status TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE runs (
    run_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    status TEXT NOT NULL,
    version INTEGER NOT NULL,
    snapshot_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY(task_id) REFERENCES tasks(task_id)
);

CREATE TABLE run_events (
    event_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    occurred_at TEXT NOT NULL,
    UNIQUE(run_id, sequence),
    FOREIGN KEY(run_id) REFERENCES runs(run_id)
);

CREATE TABLE artifacts (
    artifact_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    metadata_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE command_receipts (
    command_id TEXT PRIMARY KEY,
    command_fingerprint TEXT NOT NULL,
    result_kind TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX idx_tasks_workspace_status
    ON tasks(workspace_id, status);

CREATE INDEX idx_runs_task_status
    ON runs(task_id, status);

CREATE INDEX idx_run_events_run_sequence
    ON run_events(run_id, sequence);
