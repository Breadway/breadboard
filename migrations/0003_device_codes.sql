-- OAuth-style device authorization flow for the CLI/TUI.
--
--   1. CLI  -> POST /auth/device            (gets device_code + user_code + URL)
--   2. user -> opens URL in a browser, signs in via Forgejo, approves
--   3. CLI  -> POST /auth/device/token      (polls; gets a bearer token once approved)

CREATE TABLE device_codes (
    device_code TEXT    PRIMARY KEY,           -- opaque, CLI-held
    user_code   TEXT    NOT NULL UNIQUE,       -- short, human-entered/confirmed
    created_at  TEXT    NOT NULL,
    expires_at  TEXT    NOT NULL,
    approved_by INTEGER REFERENCES users(id) ON DELETE CASCADE,
    -- The minted bearer token, set on approval, cleared once the CLI collects it.
    token       TEXT
);

CREATE INDEX idx_device_codes_user_code ON device_codes(user_code);
