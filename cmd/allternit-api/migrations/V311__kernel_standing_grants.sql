-- AllternitOS Stage 1: standing per-tool "Allow" grants. A person's Customize per-tool policy
-- (Allow) is a standing human decision; the kernel check reads it for ask-class tools.
-- Human-only classes never read this table. Additive only.
CREATE TABLE IF NOT EXISTS kernel_standing_grants (
    owner_user_id TEXT NOT NULL,
    tool          TEXT NOT NULL,
    set_at        TEXT NOT NULL,
    PRIMARY KEY (owner_user_id, tool)
);
