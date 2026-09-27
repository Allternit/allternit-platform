-- Settings → Cowork (parity with Claude's Cowork settings).
--
-- cowork_devices: every client that talks to this API for a user. Desktop
-- apps register themselves on launch and are trusted automatically; other
-- clients (the phone PWA, another browser) appear untrusted until the user
-- trusts them. With require_trusted_devices on, /remote-control/* refuses
-- untrusted devices (`src/cowork_devices_routes.rs`).
CREATE TABLE IF NOT EXISTS cowork_devices (
    id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    name TEXT NOT NULL,
    -- macOS | Windows | Linux | iOS | Android | Web
    platform TEXT NOT NULL,
    -- desktop | browser | mobile
    kind TEXT NOT NULL DEFAULT 'desktop',
    trusted INTEGER NOT NULL DEFAULT 0,
    added_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    PRIMARY KEY (user_id, id)
);
CREATE INDEX IF NOT EXISTS idx_cowork_devices_user ON cowork_devices(user_id, last_seen_at);

ALTER TABLE user_cowork_preferences ADD COLUMN require_trusted_devices INTEGER NOT NULL DEFAULT 0;
-- Absolute folder Cowork sessions work in when no project/folder is chosen.
ALTER TABLE user_cowork_preferences ADD COLUMN files_location TEXT;
-- built-in (the app's browser) | chrome (the Allternit extension in Chrome)
ALTER TABLE user_cowork_preferences ADD COLUMN preferred_browser TEXT NOT NULL DEFAULT 'built-in';
ALTER TABLE user_cowork_preferences ADD COLUMN open_links_in_app INTEGER NOT NULL DEFAULT 1;
-- JSON array of hostnames the browser tools may use without a prompt.
ALTER TABLE user_cowork_preferences ADD COLUMN allowed_sites TEXT NOT NULL DEFAULT '[]';
