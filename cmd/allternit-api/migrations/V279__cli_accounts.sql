-- Extra signed-in accounts for a coding CLI (Claude Code, Codex). The default
-- account is implicit (the CLI's stock home, no row); a row is an added account
-- with its own home under ~/.allternit/cli-homes/<id>, which the CLI is pointed
-- at through the catalog's accountHomeEnv (CLAUDE_CONFIG_DIR, CODEX_HOME).
-- is_default = 1 on a row makes that account the user's default for the CLI.
CREATE TABLE IF NOT EXISTS cli_accounts (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    tool_id TEXT NOT NULL,
    label TEXT NOT NULL,
    home_dir TEXT NOT NULL,
    identity TEXT,
    is_default INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(user_id, tool_id, label)
);
CREATE INDEX IF NOT EXISTS idx_cli_accounts_user_tool ON cli_accounts(user_id, tool_id);
