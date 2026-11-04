-- Idempotent SMS sends: a client retry carrying the same Idempotency-Key must not text twice.
-- 'sending' rows are inserted before the carrier call; 'sent' rows hold the carrier message id.
ALTER TABLE public.sms_outbound_log ADD COLUMN IF NOT EXISTS idempotency_key text;
ALTER TABLE public.sms_outbound_log ADD COLUMN IF NOT EXISTS state text NOT NULL DEFAULT 'sent';
CREATE UNIQUE INDEX IF NOT EXISTS uq_sms_outbound_log_idem
    ON public.sms_outbound_log (number_id, idempotency_key) WHERE idempotency_key IS NOT NULL;
