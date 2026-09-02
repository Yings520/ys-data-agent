ALTER TABLE run_provider_bindings
ADD COLUMN activation_revision INTEGER NOT NULL DEFAULT 1 CHECK (activation_revision > 0);

DROP TRIGGER run_provider_binding_requires_current_active_snapshot;

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
      AND active.activation_revision = NEW.activation_revision
)
BEGIN
    SELECT RAISE(ABORT, 'Run Provider binding requires the current active snapshot');
END;
