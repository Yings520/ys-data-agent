CREATE TABLE provider_profiles (
    profile_id TEXT PRIMARY KEY,
    name TEXT NOT NULL COLLATE NOCASE UNIQUE,
    current_revision INTEGER NOT NULL CHECK (current_revision > 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (profile_id, current_revision)
        REFERENCES provider_profile_revisions(profile_id, revision)
        DEFERRABLE INITIALLY DEFERRED
);

CREATE TABLE provider_credential_generations (
    profile_id TEXT NOT NULL,
    generation INTEGER NOT NULL CHECK (generation > 0),
    kind TEXT NOT NULL CHECK (kind IN ('api_key', 'o_auth_connection')),
    vault_locator TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL CHECK (status IN ('available', 'retained', 'expired', 'revoked', 'deleted')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (profile_id, generation),
    FOREIGN KEY (profile_id) REFERENCES provider_profiles(profile_id)
        DEFERRABLE INITIALLY DEFERRED
);

CREATE TABLE provider_profile_revisions (
    profile_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    provider TEXT NOT NULL CHECK (provider IN (
        'chat_gpt_subscription', 'open_code_go', 'open_code_zen', 'deep_seek',
        'xai', 'zai', 'open_router', 'mini_max', 'anthropic'
    )),
    model_id TEXT NOT NULL CHECK (
        (provider = 'chat_gpt_subscription' AND model_id GLOB 'chatgpt/?*')
        OR (provider = 'open_code_go' AND model_id GLOB 'opencode-go/?*')
        OR (provider = 'open_code_zen' AND model_id GLOB 'opencode/?*')
        OR (provider = 'deep_seek' AND model_id GLOB 'deepseek/?*')
        OR (provider = 'xai' AND model_id GLOB 'xai/?*')
        OR (provider = 'zai' AND model_id GLOB 'zai/?*')
        OR (provider = 'open_router' AND model_id GLOB 'openrouter/?*')
        OR (provider = 'mini_max' AND model_id GLOB 'minimax/?*')
        OR (provider = 'anthropic' AND model_id GLOB 'anthropic/?*')
    ),
    parameters_json TEXT NOT NULL CHECK (
        json_valid(parameters_json)
        AND json_type(parameters_json, '$.schema_version') = 'integer'
        AND json_extract(parameters_json, '$.schema_version') = 1
    ),
    credential_generation INTEGER,
    state TEXT NOT NULL CHECK (state IN ('draft', 'ready', 'invalid')),
    validation_id TEXT,
    created_at TEXT NOT NULL,
    PRIMARY KEY (profile_id, revision),
    FOREIGN KEY (profile_id) REFERENCES provider_profiles(profile_id)
        DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (profile_id, credential_generation)
        REFERENCES provider_credential_generations(profile_id, generation)
        DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (profile_id, revision, validation_id)
        REFERENCES provider_validations(profile_id, revision, validation_id)
        DEFERRABLE INITIALLY DEFERRED,
    CHECK ((state = 'ready') = (validation_id IS NOT NULL))
);

CREATE TABLE provider_validations (
    validation_id TEXT PRIMARY KEY,
    profile_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    credential_generation INTEGER,
    validation_digest TEXT NOT NULL UNIQUE,
    tool_calls_supported INTEGER NOT NULL CHECK (tool_calls_supported IN (0, 1)),
    non_empty_tool_call_ids INTEGER NOT NULL CHECK (non_empty_tool_call_ids IN (0, 1)),
    multi_turn_tool_results INTEGER NOT NULL CHECK (multi_turn_tool_results IN (0, 1)),
    context_limit INTEGER CHECK (context_limit > 0),
    outcome TEXT NOT NULL CHECK (outcome IN ('passed', 'failed')),
    error_code TEXT,
    evidence_schema_version INTEGER NOT NULL CHECK (evidence_schema_version = 1),
    checked_at TEXT NOT NULL,
    UNIQUE (profile_id, revision, validation_id),
    UNIQUE (profile_id, revision, credential_generation, validation_id, validation_digest),
    FOREIGN KEY (profile_id, revision)
        REFERENCES provider_profile_revisions(profile_id, revision)
        DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (profile_id, credential_generation)
        REFERENCES provider_credential_generations(profile_id, generation)
        DEFERRABLE INITIALLY DEFERRED,
    CHECK ((outcome = 'passed') = (error_code IS NULL)),
    CHECK (
        outcome <> 'passed' OR (
            tool_calls_supported = 1
            AND non_empty_tool_call_ids = 1
            AND multi_turn_tool_results = 1
            AND context_limit IS NOT NULL
        )
    )
);

CREATE TABLE active_provider (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    profile_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    validation_id TEXT NOT NULL,
    credential_generation INTEGER NOT NULL CHECK (credential_generation > 0),
    validation_digest TEXT NOT NULL,
    activation_revision INTEGER NOT NULL CHECK (activation_revision > 0),
    activated_at TEXT NOT NULL,
    FOREIGN KEY (profile_id, revision)
        REFERENCES provider_profile_revisions(profile_id, revision),
    FOREIGN KEY (validation_id) REFERENCES provider_validations(validation_id),
    FOREIGN KEY (profile_id, credential_generation)
        REFERENCES provider_credential_generations(profile_id, generation),
    FOREIGN KEY (
        profile_id, revision, credential_generation, validation_id, validation_digest
    ) REFERENCES provider_validations(
        profile_id, revision, credential_generation, validation_id, validation_digest
    )
);

CREATE TABLE credential_mutations (
    mutation_id TEXT PRIMARY KEY,
    profile_id TEXT NOT NULL,
    old_generation INTEGER,
    new_generation INTEGER,
    rollback_generation INTEGER,
    operation TEXT NOT NULL CHECK (operation IN ('create', 'replace', 'refresh', 'delete', 'revoke')),
    phase TEXT NOT NULL CHECK (phase IN (
        'intent_recorded', 'vault_written', 'pointer_committed', 'cleanup_pending',
        'rolled_back', 'completed', 'blocked'
    )),
    error_code TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (profile_id) REFERENCES provider_profiles(profile_id)
        DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (profile_id, old_generation)
        REFERENCES provider_credential_generations(profile_id, generation)
        DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (profile_id, new_generation)
        REFERENCES provider_credential_generations(profile_id, generation)
        DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (profile_id, rollback_generation)
        REFERENCES provider_credential_generations(profile_id, generation)
        DEFERRABLE INITIALLY DEFERRED
);

CREATE TABLE run_provider_bindings (
    run_id TEXT PRIMARY KEY,
    profile_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    provider TEXT NOT NULL CHECK (provider IN (
        'chat_gpt_subscription', 'open_code_go', 'open_code_zen', 'deep_seek',
        'xai', 'zai', 'open_router', 'mini_max', 'anthropic'
    )),
    model_id TEXT NOT NULL CHECK (
        (provider = 'chat_gpt_subscription' AND model_id GLOB 'chatgpt/?*')
        OR (provider = 'open_code_go' AND model_id GLOB 'opencode-go/?*')
        OR (provider = 'open_code_zen' AND model_id GLOB 'opencode/?*')
        OR (provider = 'deep_seek' AND model_id GLOB 'deepseek/?*')
        OR (provider = 'xai' AND model_id GLOB 'xai/?*')
        OR (provider = 'zai' AND model_id GLOB 'zai/?*')
        OR (provider = 'open_router' AND model_id GLOB 'openrouter/?*')
        OR (provider = 'mini_max' AND model_id GLOB 'minimax/?*')
        OR (provider = 'anthropic' AND model_id GLOB 'anthropic/?*')
    ),
    parameters_json TEXT NOT NULL CHECK (
        json_valid(parameters_json)
        AND json_type(parameters_json, '$.schema_version') = 'integer'
        AND json_extract(parameters_json, '$.schema_version') = 1
    ),
    credential_generation INTEGER NOT NULL CHECK (credential_generation > 0),
    validation_id TEXT NOT NULL,
    validation_digest TEXT NOT NULL,
    fingerprint_json TEXT NOT NULL CHECK (
        json_valid(fingerprint_json)
        AND json_type(fingerprint_json, '$.schema_version') = 'integer'
        AND json_extract(fingerprint_json, '$.schema_version') = 1
    ),
    fingerprint_hash TEXT NOT NULL CHECK (
        length(fingerprint_hash) = 64
        AND fingerprint_hash = lower(fingerprint_hash)
        AND fingerprint_hash NOT GLOB '*[^0-9a-f]*'
    ),
    created_at TEXT NOT NULL,
    FOREIGN KEY (run_id) REFERENCES runs(run_id),
    FOREIGN KEY (profile_id, revision)
        REFERENCES provider_profile_revisions(profile_id, revision),
    FOREIGN KEY (profile_id, credential_generation)
        REFERENCES provider_credential_generations(profile_id, generation),
    FOREIGN KEY (validation_id) REFERENCES provider_validations(validation_id),
    FOREIGN KEY (
        profile_id, revision, credential_generation, validation_id, validation_digest
    ) REFERENCES provider_validations(
        profile_id, revision, credential_generation, validation_id, validation_digest
    )
);

CREATE INDEX idx_provider_profile_revisions_latest
    ON provider_profile_revisions(profile_id, revision DESC);

CREATE INDEX idx_credential_mutations_phase
    ON credential_mutations(phase);

CREATE INDEX idx_run_provider_bindings_profile_credential
    ON run_provider_bindings(profile_id, credential_generation);

CREATE TRIGGER active_provider_requires_matching_ready_validation
BEFORE INSERT ON active_provider
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM provider_profile_revisions AS revision
    JOIN provider_validations AS validation
      ON validation.validation_id = NEW.validation_id
     AND validation.profile_id = NEW.profile_id
     AND validation.revision = NEW.revision
     AND validation.credential_generation = NEW.credential_generation
     AND validation.validation_digest = NEW.validation_digest
    WHERE revision.profile_id = NEW.profile_id
      AND revision.revision = NEW.revision
      AND revision.state = 'ready'
      AND revision.validation_id = NEW.validation_id
      AND validation.outcome = 'passed'
)
BEGIN
    SELECT RAISE(ABORT, 'active Provider must reference matching ready validation');
END;

CREATE TRIGGER active_provider_update_requires_matching_ready_validation
BEFORE UPDATE OF profile_id, revision, validation_id, credential_generation, validation_digest ON active_provider
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM provider_profile_revisions AS revision
    JOIN provider_validations AS validation
      ON validation.validation_id = NEW.validation_id
     AND validation.profile_id = NEW.profile_id
     AND validation.revision = NEW.revision
     AND validation.credential_generation = NEW.credential_generation
     AND validation.validation_digest = NEW.validation_digest
    WHERE revision.profile_id = NEW.profile_id
      AND revision.revision = NEW.revision
      AND revision.state = 'ready'
      AND revision.validation_id = NEW.validation_id
      AND validation.outcome = 'passed'
)
BEGIN
    SELECT RAISE(ABORT, 'active Provider must reference matching ready validation');
END;

CREATE TRIGGER provider_profile_revisions_configuration_is_immutable
BEFORE UPDATE OF profile_id, revision, provider, model_id, parameters_json, credential_generation
ON provider_profile_revisions
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'Provider revision configuration is immutable');
END;

CREATE TRIGGER provider_credential_generations_identity_is_immutable
BEFORE UPDATE OF profile_id, generation, kind, vault_locator ON provider_credential_generations
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'Provider credential generations are immutable');
END;

CREATE TRIGGER provider_credential_generations_insert_only_delete
BEFORE DELETE ON provider_credential_generations
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'Provider credential generations are insert-only');
END;

CREATE TRIGGER provider_profile_revisions_insert_only_delete
BEFORE DELETE ON provider_profile_revisions
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'Provider revisions are insert-only');
END;

CREATE TRIGGER provider_validations_insert_only_update
BEFORE UPDATE ON provider_validations
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'Provider validations are insert-only');
END;

CREATE TRIGGER provider_validations_insert_only_delete
BEFORE DELETE ON provider_validations
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'Provider validations are insert-only');
END;

CREATE TRIGGER provider_profile_revisions_append_in_order
BEFORE INSERT ON provider_profile_revisions
FOR EACH ROW
WHEN NEW.revision <> COALESCE((
    SELECT MAX(revision) + 1
    FROM provider_profile_revisions
    WHERE profile_id = NEW.profile_id
), 1)
BEGIN
    SELECT RAISE(ABORT, 'Provider revisions must append in order');
END;

CREATE TRIGGER provider_profile_revisions_require_matching_credential_kind
BEFORE INSERT ON provider_profile_revisions
FOR EACH ROW
WHEN NEW.credential_generation IS NOT NULL
 AND NOT EXISTS (
    SELECT 1
    FROM provider_credential_generations AS credential
    WHERE credential.profile_id = NEW.profile_id
      AND credential.generation = NEW.credential_generation
      AND (
          (NEW.provider = 'chat_gpt_subscription' AND credential.kind = 'o_auth_connection')
          OR (NEW.provider <> 'chat_gpt_subscription' AND credential.kind = 'api_key')
      )
 )
BEGIN
    SELECT RAISE(ABORT, 'Provider credential kind must match the Provider');
END;

CREATE TRIGGER provider_parameters_are_non_sensitive_on_insert
BEFORE INSERT ON provider_profile_revisions
FOR EACH ROW
WHEN json_type(NEW.parameters_json) <> 'object'
  OR EXISTS (
      SELECT 1 FROM json_each(NEW.parameters_json)
      WHERE key NOT IN (
          'schema_version', 'temperature', 'max_tokens', 'timeout_seconds',
          'retry_count', 'provider_specific'
      )
  )
  OR (json_type(NEW.parameters_json, '$.provider_specific') IS NOT NULL
      AND json_type(NEW.parameters_json, '$.provider_specific') <> 'object')
  OR (json_type(NEW.parameters_json, '$.temperature') IS NOT NULL
      AND json_type(NEW.parameters_json, '$.temperature') NOT IN ('null', 'integer', 'real'))
  OR (json_type(NEW.parameters_json, '$.max_tokens') IS NOT NULL
      AND json_type(NEW.parameters_json, '$.max_tokens') NOT IN ('null', 'integer'))
  OR (json_type(NEW.parameters_json, '$.timeout_seconds') IS NOT NULL
      AND json_type(NEW.parameters_json, '$.timeout_seconds') <> 'integer')
  OR (json_type(NEW.parameters_json, '$.retry_count') IS NOT NULL
      AND json_type(NEW.parameters_json, '$.retry_count') <> 'integer')
  OR EXISTS (
      SELECT 1 FROM json_each(NEW.parameters_json, '$.provider_specific')
      WHERE type NOT IN ('true', 'false', 'integer')
  )
BEGIN
    SELECT RAISE(ABORT, 'Provider parameters must use the non-sensitive schema');
END;

CREATE TRIGGER provider_profile_revisions_ready_requires_passing_validation
BEFORE INSERT ON provider_profile_revisions
FOR EACH ROW
WHEN NEW.state = 'ready'
 AND (NEW.credential_generation IS NULL OR NOT EXISTS (
    SELECT 1
    FROM provider_validations AS validation
    WHERE validation.validation_id = NEW.validation_id
      AND validation.profile_id = NEW.profile_id
      AND validation.revision = NEW.revision
      AND validation.credential_generation IS NEW.credential_generation
      AND validation.outcome = 'passed'
 ))
BEGIN
    SELECT RAISE(ABORT, 'Ready Provider revision requires matching passing validation');
END;

CREATE TRIGGER provider_profile_revisions_ready_update_requires_passing_validation
BEFORE UPDATE OF state, validation_id ON provider_profile_revisions
FOR EACH ROW
WHEN NEW.state = 'ready'
 AND (NEW.credential_generation IS NULL OR NOT EXISTS (
    SELECT 1
    FROM provider_validations AS validation
    WHERE validation.validation_id = NEW.validation_id
      AND validation.profile_id = NEW.profile_id
      AND validation.revision = NEW.revision
      AND validation.credential_generation IS NEW.credential_generation
      AND validation.outcome = 'passed'
 ))
BEGIN
    SELECT RAISE(ABORT, 'Ready Provider revision requires matching passing validation');
END;

CREATE TRIGGER provider_parameters_are_non_sensitive_on_update
BEFORE UPDATE OF parameters_json ON provider_profile_revisions
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'Provider revision configuration is immutable');
END;

CREATE TRIGGER run_provider_binding_fingerprint_is_non_sensitive
BEFORE INSERT ON run_provider_bindings
FOR EACH ROW
WHEN json_type(NEW.fingerprint_json) <> 'object'
  OR EXISTS (
      SELECT 1 FROM json_each(NEW.fingerprint_json)
      WHERE key NOT IN (
          'schema_version', 'profile_id', 'profile_revision', 'provider', 'model', 'parameters'
      )
  )
  OR json_type(NEW.fingerprint_json, '$.parameters') <> 'object'
  OR EXISTS (
      SELECT 1 FROM json_each(NEW.fingerprint_json, '$.parameters')
      WHERE key NOT IN ('temperature', 'max_tokens', 'timeout_seconds', 'retry_count')
         OR type NOT IN ('null', 'integer', 'real')
  )
  OR json_type(NEW.fingerprint_json, '$.parameters.temperature') IS NULL
  OR json_type(NEW.fingerprint_json, '$.parameters.max_tokens') IS NULL
  OR json_type(NEW.fingerprint_json, '$.parameters.timeout_seconds') IS NULL
  OR json_type(NEW.fingerprint_json, '$.parameters.retry_count') IS NULL
  OR json_type(NEW.fingerprint_json, '$.parameters.temperature')
     IS NOT json_type(NEW.parameters_json, '$.temperature')
  OR json_extract(NEW.fingerprint_json, '$.parameters.temperature')
     IS NOT json_extract(NEW.parameters_json, '$.temperature')
  OR json_type(NEW.fingerprint_json, '$.parameters.max_tokens')
     IS NOT json_type(NEW.parameters_json, '$.max_tokens')
  OR json_extract(NEW.fingerprint_json, '$.parameters.max_tokens')
     IS NOT json_extract(NEW.parameters_json, '$.max_tokens')
  OR json_type(NEW.fingerprint_json, '$.parameters.timeout_seconds')
     IS NOT json_type(NEW.parameters_json, '$.timeout_seconds')
  OR json_extract(NEW.fingerprint_json, '$.parameters.timeout_seconds')
     IS NOT json_extract(NEW.parameters_json, '$.timeout_seconds')
  OR json_type(NEW.fingerprint_json, '$.parameters.retry_count')
     IS NOT json_type(NEW.parameters_json, '$.retry_count')
  OR json_extract(NEW.fingerprint_json, '$.parameters.retry_count')
     IS NOT json_extract(NEW.parameters_json, '$.retry_count')
  OR json_type(NEW.fingerprint_json, '$.profile_id') <> 'text'
  OR json_extract(NEW.fingerprint_json, '$.profile_id') <> NEW.profile_id
  OR json_type(NEW.fingerprint_json, '$.profile_revision') <> 'integer'
  OR json_extract(NEW.fingerprint_json, '$.profile_revision') <> NEW.revision
  OR json_type(NEW.fingerprint_json, '$.provider') <> 'text'
  OR json_extract(NEW.fingerprint_json, '$.provider') <> NEW.provider
  OR json_type(NEW.fingerprint_json, '$.model') <> 'object'
  OR EXISTS (
      SELECT 1 FROM json_each(NEW.fingerprint_json, '$.model')
      WHERE key NOT IN ('provider', 'value') OR type <> 'text'
  )
  OR json_type(NEW.fingerprint_json, '$.model.provider') <> 'text'
  OR json_type(NEW.fingerprint_json, '$.model.value') <> 'text'
  OR json_extract(NEW.fingerprint_json, '$.model.provider') IS NOT NEW.provider
  OR json_extract(NEW.fingerprint_json, '$.model.value') IS NOT NEW.model_id
BEGIN
    SELECT RAISE(ABORT, 'Provider fingerprint must use the non-sensitive schema');
END;

CREATE TRIGGER run_provider_binding_requires_passing_validation
BEFORE INSERT ON run_provider_bindings
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM provider_validations AS validation
    JOIN provider_profile_revisions AS revision
      ON revision.profile_id = NEW.profile_id
     AND revision.revision = NEW.revision
    WHERE validation.profile_id = NEW.profile_id
      AND validation.revision = NEW.revision
      AND validation.credential_generation = NEW.credential_generation
      AND validation.validation_id = NEW.validation_id
      AND validation.validation_digest = NEW.validation_digest
      AND validation.outcome = 'passed'
      AND revision.state = 'ready'
      AND revision.validation_id = NEW.validation_id
)
BEGIN
    SELECT RAISE(ABORT, 'Run Provider binding requires matching passing validation');
END;

CREATE TRIGGER run_provider_binding_matches_revision_snapshot
BEFORE INSERT ON run_provider_bindings
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM provider_profile_revisions AS revision
    WHERE revision.profile_id = NEW.profile_id
      AND revision.revision = NEW.revision
      AND revision.provider = NEW.provider
      AND revision.model_id = NEW.model_id
      AND revision.parameters_json = NEW.parameters_json
)
BEGIN
    SELECT RAISE(ABORT, 'Run Provider binding must match the revision snapshot');
END;

CREATE TRIGGER run_provider_binding_requires_current_active_snapshot
BEFORE INSERT ON run_provider_bindings
FOR EACH ROW
WHEN NOT EXISTS (
    SELECT 1
    FROM active_provider AS active
    WHERE active.profile_id = NEW.profile_id
      AND active.revision = NEW.revision
      AND active.validation_id = NEW.validation_id
      AND active.credential_generation = NEW.credential_generation
      AND active.validation_digest = NEW.validation_digest
)
BEGIN
    SELECT RAISE(ABORT, 'Run Provider binding requires the current active snapshot');
END;

CREATE TRIGGER run_provider_bindings_insert_only_update
BEFORE UPDATE ON run_provider_bindings
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'run Provider bindings are insert-only');
END;

CREATE TRIGGER run_provider_bindings_insert_only_delete
BEFORE DELETE ON run_provider_bindings
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'run Provider bindings are insert-only');
END;
