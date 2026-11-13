ALTER TABLE public.sandboxes
    ADD COLUMN runtime_state text CHECK (runtime_state IN ('running', 'stopped', 'missing')),
    ADD COLUMN runtime_checked_at timestamptz,
    ADD COLUMN runtime_error text;

-- One durable work item per sandbox. Inspection can be promoted to termination;
-- its lease is fenced so a late observation cannot undo cancellation.
CREATE TABLE public.sandbox_lifecycle_jobs (
    sandbox_id uuid PRIMARY KEY REFERENCES public.sandboxes(id),
    operation text NOT NULL CHECK (operation IN ('inspect', 'terminate')),
    provider_pin jsonb,
    provider_scope text NOT NULL,
    attempts integer NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at timestamptz NOT NULL DEFAULT now(),
    lease_token uuid,
    lease_expires_at timestamptz,
    last_error text,
    completed_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK ((lease_token IS NULL) = (lease_expires_at IS NULL))
);
CREATE INDEX sandbox_lifecycle_jobs_due ON public.sandbox_lifecycle_jobs(next_attempt_at)
    WHERE completed_at IS NULL;
