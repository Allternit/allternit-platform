-- Account projects (phone feedback 1.0.4, item 4; surfaces/docs/api/account-projects.mdx).
-- Projects and bots a person makes with NO computer, saved to the account so
-- every device lists them. When a computer connects it adopts the unadopted
-- rows: it creates its own cowork project under the SAME id, then marks the
-- row adopted (adopted_computer_id). After that the computer's copy wins.
-- A delete is a tombstone (deleted_at) so an offline computer still learns of
-- it. Bot setup lives in metadata.brief. Erased with the account
-- (account_erase.rs).
--
-- Prod runs cloud-api with ALLTERNIT_SKIP_MIGRATIONS=1, so the server never
-- applies this; deploy-contabo.sh applies it with psql before the binary swap
-- (ops_applied_migrations). Additive only, every statement is re-runnable.
CREATE TABLE IF NOT EXISTS account_projects (
    id                  UUID PRIMARY KEY,
    user_id             TEXT NOT NULL,
    kind                TEXT NOT NULL CHECK (kind IN ('chat', 'code', 'design', 'bot')),
    title               TEXT NOT NULL,
    description         TEXT,
    instructions        TEXT,
    metadata            JSONB NOT NULL DEFAULT '{}'::jsonb,
    adopted_computer_id TEXT,
    deleted_at          TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS account_projects_user_deleted ON account_projects (user_id, deleted_at);

-- A no-computer chat can belong to an account project.
ALTER TABLE plan_threads ADD COLUMN IF NOT EXISTS project_id UUID NULL;
