-- A failed compatibility result is durable evidence for an Invalid revision. The original v2
-- CHECK incorrectly allowed a validation reference only for Ready rows. Rebuild this table under
-- deferred FK enforcement so all existing append-only revisions and bindings retain their ids.
PRAGMA defer_foreign_keys = ON;

CREATE TABLE provider_profile_revisions_v5 (
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
    CHECK ((state = 'draft') = (validation_id IS NULL))
);

INSERT INTO provider_profile_revisions_v5(
    profile_id, revision, provider, model_id, parameters_json, credential_generation, state,
    validation_id, created_at
)
SELECT
    profile_id, revision, provider, model_id, parameters_json, credential_generation, state,
    validation_id, created_at
FROM provider_profile_revisions;

DROP TRIGGER active_provider_requires_matching_ready_validation;
DROP TRIGGER active_provider_update_requires_matching_ready_validation;
DROP TRIGGER run_provider_binding_requires_passing_validation;
DROP TRIGGER run_provider_binding_matches_revision_snapshot;
DROP TABLE provider_profile_revisions;
ALTER TABLE provider_profile_revisions_v5 RENAME TO provider_profile_revisions;

CREATE INDEX idx_provider_profile_revisions_latest
    ON provider_profile_revisions(profile_id, revision DESC);

CREATE TRIGGER provider_profile_revisions_configuration_is_immutable
BEFORE UPDATE OF profile_id, revision, provider, model_id, parameters_json, credential_generation
ON provider_profile_revisions
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'Provider revision configuration is immutable');
END;

CREATE TRIGGER provider_profile_revisions_insert_only_delete
BEFORE DELETE ON provider_profile_revisions
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'Provider revisions are insert-only');
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

CREATE TRIGGER provider_profile_revisions_invalid_requires_failed_validation
BEFORE INSERT ON provider_profile_revisions
FOR EACH ROW
WHEN NEW.state = 'invalid'
 AND NOT EXISTS (
    SELECT 1
    FROM provider_validations AS validation
    WHERE validation.validation_id = NEW.validation_id
      AND validation.profile_id = NEW.profile_id
      AND validation.revision = NEW.revision
      AND validation.credential_generation IS NEW.credential_generation
      AND validation.outcome = 'failed'
 )
BEGIN
    SELECT RAISE(ABORT, 'Invalid Provider revision requires matching failed validation');
END;

CREATE TRIGGER provider_profile_revisions_invalid_update_requires_failed_validation
BEFORE UPDATE OF state, validation_id ON provider_profile_revisions
FOR EACH ROW
WHEN NEW.state = 'invalid'
 AND NOT EXISTS (
    SELECT 1
    FROM provider_validations AS validation
    WHERE validation.validation_id = NEW.validation_id
      AND validation.profile_id = NEW.profile_id
      AND validation.revision = NEW.revision
      AND validation.credential_generation IS NEW.credential_generation
      AND validation.outcome = 'failed'
 )
BEGIN
    SELECT RAISE(ABORT, 'Invalid Provider revision requires matching failed validation');
END;

CREATE TRIGGER provider_parameters_are_non_sensitive_on_update
BEFORE UPDATE OF parameters_json ON provider_profile_revisions
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'Provider revision configuration is immutable');
END;

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
