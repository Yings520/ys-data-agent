DROP INDEX idx_credential_mutations_phase;

ALTER TABLE credential_mutations RENAME TO credential_mutations_v2;

CREATE TABLE credential_mutations (
    mutation_id TEXT PRIMARY KEY,
    profile_id TEXT NOT NULL,
    expected_revision INTEGER NOT NULL CHECK (expected_revision > 0),
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
        DEFERRABLE INITIALLY DEFERRED,
    CHECK ((phase = 'blocked') = (error_code IS NOT NULL)),
    CHECK (
        (operation = 'create'
            AND old_generation IS NULL
            AND new_generation IS NOT NULL
            AND rollback_generation IS NULL)
        OR (operation IN ('replace', 'refresh')
            AND old_generation IS NOT NULL
            AND new_generation IS NOT NULL
            AND new_generation > old_generation
            AND rollback_generation IS NULL)
        OR (operation IN ('delete', 'revoke')
            AND old_generation IS NOT NULL
            AND new_generation IS NULL
            AND rollback_generation IS NOT NULL
            AND rollback_generation > old_generation)
    )
);

INSERT INTO credential_mutations(
    mutation_id, profile_id, expected_revision, old_generation, new_generation,
    rollback_generation, operation, phase, error_code, created_at, updated_at
)
SELECT
    mutation.mutation_id,
    mutation.profile_id,
    profile.current_revision,
    mutation.old_generation,
    mutation.new_generation,
    mutation.rollback_generation,
    mutation.operation,
    CASE
        WHEN mutation.phase IN ('completed', 'rolled_back', 'blocked') THEN mutation.phase
        ELSE 'blocked'
    END,
    CASE
        WHEN mutation.phase IN ('completed', 'rolled_back') THEN NULL
        WHEN mutation.phase = 'blocked' THEN COALESCE(mutation.error_code, 'provider.storage.conflict')
        ELSE 'provider.storage.conflict'
    END,
    mutation.created_at,
    mutation.updated_at
FROM credential_mutations_v2 AS mutation
JOIN provider_profiles AS profile ON profile.profile_id = mutation.profile_id;

DELETE FROM active_provider
WHERE profile_id IN (
    SELECT profile_id FROM credential_mutations WHERE phase = 'blocked'
);

DROP TABLE credential_mutations_v2;

CREATE INDEX idx_credential_mutations_phase
    ON credential_mutations(phase);
