-- Coding tools a user added themselves (Settings → Connections → Coding
-- tools → "Add a CLI"), `src/cli_catalog.rs`. Every argument list is a JSON
-- array of strings run as argv — never a shell string. `prompt_args` holds
-- exactly one "{prompt}" element (the prompt is one argv element);
-- `model_args` may hold "{model}". The rows are mirrored to
-- <harness home>/custom-clis.json, which gizzi's subprocess discovery reads,
-- so a custom tool runs as a brain through the same channel as built-in CLIs.
CREATE TABLE IF NOT EXISTS custom_cli_tools (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    name TEXT NOT NULL,
    command TEXT NOT NULL,
    prompt_args TEXT NOT NULL,
    model_args TEXT,
    version_args TEXT,
    auth_check_args TEXT,
    install_command TEXT,
    homepage TEXT,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_custom_cli_tools_user ON custom_cli_tools(user_id, created_at);
