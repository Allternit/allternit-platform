-- Bot email provisioned through Allternit's cloud (no admin key on this runtime):
-- each mailbox has its own inbound webhook signing secret and the mail service URL
-- it was created on.
ALTER TABLE agent_identity_channels ADD COLUMN email_webhook_secret_sealed TEXT;
ALTER TABLE agent_identity_channels ADD COLUMN email_mail_url TEXT;
