-- Keep immutable Run bindings interpretable after a Profile leaves the management surface.
ALTER TABLE provider_profiles ADD COLUMN deleted_at TEXT;

CREATE INDEX idx_provider_profiles_visible
    ON provider_profiles(deleted_at, name COLLATE NOCASE);
