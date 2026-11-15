-- Worker bots: `owns` is a JSON string list of the kinds of tasks a bot owns
-- (e.g. ["invoices","code review"]). Teammates see it in /bot-routing/roster
-- and hand that work to the owning bot instead of doing it themselves.
ALTER TABLE agents ADD COLUMN owns TEXT;
