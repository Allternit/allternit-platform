-- V1 baseline creates memory_entities without `summary`; V86 (memory kernel
-- v2) uses CREATE TABLE IF NOT EXISTS, so its column list never applied.
-- Every entity list and every recall then failed with "no such column:
-- summary", which silently disabled memory recall for all chats.
ALTER TABLE memory_entities ADD COLUMN summary TEXT;
