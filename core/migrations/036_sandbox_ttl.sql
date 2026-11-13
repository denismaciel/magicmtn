ALTER TABLE public.sandboxes
    ADD COLUMN ttl_seconds integer,
    ADD COLUMN expires_at timestamptz,
    ADD CONSTRAINT sandbox_ttl_pair CHECK ((ttl_seconds IS NULL) = (expires_at IS NULL)),
    ADD CONSTRAINT sandbox_ttl_positive CHECK (ttl_seconds IS NULL OR ttl_seconds > 0);

ALTER TABLE public.chat_sessions
    ADD COLUMN sandbox_ttl_seconds integer,
    ADD COLUMN sandbox_expires_at timestamptz,
    ADD CONSTRAINT chat_sandbox_ttl_pair CHECK ((sandbox_ttl_seconds IS NULL) = (sandbox_expires_at IS NULL));

CREATE INDEX sandboxes_expiry ON public.sandboxes(expires_at, id)
    WHERE expires_at IS NOT NULL AND status IS DISTINCT FROM 'ended';
