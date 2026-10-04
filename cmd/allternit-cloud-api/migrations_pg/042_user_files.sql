-- Files users attach in Allternit, stored in the private R2 bucket
-- allternit-user-files at u/<userId>/<fileId>/<name>. A row exists only after
-- the upload was verified (HEAD size == declared size); pending uploads leave no row.
CREATE TABLE IF NOT EXISTS user_files (
    id uuid PRIMARY KEY,
    user_id text NOT NULL,
    key text NOT NULL UNIQUE,
    name text NOT NULL,
    content_type text NOT NULL,
    bytes bigint NOT NULL CHECK (bytes > 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    deleted_at timestamptz
);
CREATE INDEX IF NOT EXISTS idx_user_files_user_live ON user_files (user_id) WHERE deleted_at IS NULL;
