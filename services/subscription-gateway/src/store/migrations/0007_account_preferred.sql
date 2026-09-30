-- One preferred account per provider (Eoj: switch between two logins of the
-- same subscription). The router picks it first among equally healthy
-- accounts; a limited or signed-out preferred account still falls back.
ALTER TABLE accounts ADD COLUMN preferred INTEGER NOT NULL DEFAULT 0;
-- Existing installs: the first account of each provider becomes preferred.
UPDATE accounts SET preferred = 1
WHERE account_id IN (SELECT MIN(account_id) FROM accounts GROUP BY provider);
