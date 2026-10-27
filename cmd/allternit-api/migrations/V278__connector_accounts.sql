-- Multiple accounts per connector (e.g. two Gmail logins for one user).
-- connector_connections was UNIQUE(connector_id, user_id); SQLite cannot alter
-- a table constraint, so rebuild: new table, copy, drop, rename, indexes.
-- account_key is the provider identity (email/login, lower-cased); '' only for
-- legacy rows until their first refresh. Every existing row becomes the
-- default account of its (connector_id, user_id).

CREATE TABLE connector_connections_v2 (
    id            TEXT PRIMARY KEY,
    connector_id  TEXT NOT NULL,
    user_id       TEXT,
    auth_type     TEXT NOT NULL,
    status        TEXT NOT NULL DEFAULT 'disconnected',
    access_token  TEXT,
    refresh_token TEXT,
    expires_at    DATETIME,
    scopes        TEXT,
    account       TEXT,
    metadata      TEXT,
    created_at    DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at    DATETIME DEFAULT CURRENT_TIMESTAMP,
    backend       TEXT NOT NULL DEFAULT 'rust_native',
    account_key   TEXT NOT NULL DEFAULT '',
    is_default    INTEGER NOT NULL DEFAULT 0,
    UNIQUE(connector_id, user_id, account_key)
);

INSERT INTO connector_connections_v2
    (id, connector_id, user_id, auth_type, status, access_token, refresh_token,
     expires_at, scopes, account, metadata, created_at, updated_at, backend,
     account_key, is_default)
SELECT id, connector_id, user_id, auth_type, status, access_token, refresh_token,
       expires_at, scopes, account, metadata, created_at, updated_at, backend,
       '', 1
FROM connector_connections;

DROP TABLE connector_connections;
ALTER TABLE connector_connections_v2 RENAME TO connector_connections;

CREATE INDEX IF NOT EXISTS idx_connector_connections_user ON connector_connections(user_id);
CREATE INDEX IF NOT EXISTS idx_connector_connections_status ON connector_connections(status);
