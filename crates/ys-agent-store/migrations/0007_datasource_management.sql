CREATE TABLE datasource_workspaces (
    workspace_id TEXT PRIMARY KEY,
    version INTEGER NOT NULL CHECK(version >= 0)
);
CREATE TABLE datasource_profiles (
    workspace_id TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    name_key TEXT NOT NULL,
    head_revision INTEGER NOT NULL CHECK(head_revision > 0),
    deleted_at TEXT,
    profile_json TEXT NOT NULL,
    PRIMARY KEY(workspace_id, profile_id),
    FOREIGN KEY(workspace_id) REFERENCES datasource_workspaces(workspace_id),
    FOREIGN KEY(workspace_id, profile_id, head_revision)
        REFERENCES datasource_revisions(workspace_id, profile_id, revision)
        DEFERRABLE INITIALLY DEFERRED
);
CREATE UNIQUE INDEX datasource_visible_names ON datasource_profiles(workspace_id, name_key)
    WHERE deleted_at IS NULL;
CREATE TABLE datasource_credential_generations (
    workspace_id TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    generation INTEGER NOT NULL CHECK(generation > 0),
    state TEXT NOT NULL CHECK(state IN ('prepared', 'available', 'retired', 'removing', 'removed')),
    PRIMARY KEY(workspace_id, profile_id, generation)
);
CREATE TABLE datasource_revisions (
    workspace_id TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK(revision > 0),
    generation INTEGER,
    revision_json TEXT NOT NULL,
    PRIMARY KEY(workspace_id, profile_id, revision),
    FOREIGN KEY(workspace_id, profile_id) REFERENCES datasource_profiles(workspace_id, profile_id),
    FOREIGN KEY(workspace_id, profile_id, generation)
        REFERENCES datasource_credential_generations(workspace_id, profile_id, generation)
);
CREATE TRIGGER datasource_revisions_immutable BEFORE UPDATE ON datasource_revisions
    BEGIN SELECT RAISE(ABORT, 'immutable datasource revision'); END;
CREATE TABLE datasource_validations (
    workspace_id TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    validation_id TEXT PRIMARY KEY,
    evidence_json TEXT NOT NULL,
    UNIQUE(workspace_id, profile_id, revision, validation_id),
    FOREIGN KEY(workspace_id, profile_id, revision)
        REFERENCES datasource_revisions(workspace_id, profile_id, revision)
);
CREATE TRIGGER datasource_validations_immutable BEFORE UPDATE ON datasource_validations
    BEGIN SELECT RAISE(ABORT, 'immutable datasource evidence'); END;
CREATE TABLE datasource_revision_states (
    workspace_id TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    state_json TEXT NOT NULL,
    validation_id TEXT,
    PRIMARY KEY(workspace_id, profile_id, revision),
    FOREIGN KEY(workspace_id, profile_id, revision, validation_id)
        REFERENCES datasource_validations(workspace_id, profile_id, revision, validation_id),
    FOREIGN KEY(workspace_id, profile_id, revision)
        REFERENCES datasource_revisions(workspace_id, profile_id, revision)
);
CREATE TABLE datasource_selections (
    workspace_id TEXT NOT NULL,
    selection_kind TEXT NOT NULL CHECK(selection_kind IN ('session', 'default')),
    owner_id TEXT NOT NULL,
    profile_id TEXT,
    revision INTEGER,
    version INTEGER NOT NULL CHECK(version > 0),
    PRIMARY KEY(workspace_id, selection_kind, owner_id),
    CHECK((profile_id IS NULL) = (revision IS NULL)),
    FOREIGN KEY(workspace_id, profile_id, revision)
        REFERENCES datasource_revisions(workspace_id, profile_id, revision)
);
CREATE TABLE datasource_secret_journal (
    mutation_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    phase TEXT NOT NULL CHECK(phase IN ('prepared', 'vault_written', 'committed')),
    mutation_json TEXT NOT NULL,
    UNIQUE(workspace_id, profile_id)
);
CREATE TABLE datasource_command_receipts (
    command_id TEXT PRIMARY KEY,
    request_json TEXT NOT NULL,
    receipt_json TEXT NOT NULL
);
CREATE TABLE run_datasource_bindings (
    run_id TEXT PRIMARY KEY REFERENCES runs(run_id),
    workspace_id TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    generation INTEGER,
    binding_json TEXT NOT NULL,
    FOREIGN KEY(workspace_id, profile_id, revision)
        REFERENCES datasource_revisions(workspace_id, profile_id, revision),
    FOREIGN KEY(workspace_id, profile_id, generation)
        REFERENCES datasource_credential_generations(workspace_id, profile_id, generation)
);
CREATE INDEX run_datasource_profile ON run_datasource_bindings(workspace_id, profile_id);
CREATE TRIGGER run_datasource_bindings_immutable BEFORE UPDATE ON run_datasource_bindings
    BEGIN SELECT RAISE(ABORT, 'immutable datasource binding'); END;
