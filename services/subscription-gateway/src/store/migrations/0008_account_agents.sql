-- Agents the account has on its provider (e.g. Claude Projects), read after sign-in: JSON [{id, name, kind}].
ALTER TABLE accounts ADD COLUMN agents TEXT;
