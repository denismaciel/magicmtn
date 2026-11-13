-- Current provider storage is owned by the engine. Cloud databases already have
-- these relations from migration 012; preserve their constraints and data.
CREATE TABLE IF NOT EXISTS public.provider_runners (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    organization_id uuid NOT NULL DEFAULT magicmtn_current_organization(),
    display_name text NOT NULL,
    token_sha256 text NOT NULL UNIQUE CHECK (token_sha256 ~ '^[a-f0-9]{64}$'),
    last_seen_at timestamptz,
    revoked_at timestamptz,
    UNIQUE(organization_id,id)
);
CREATE TABLE IF NOT EXISTS public.provider_connections (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    organization_id uuid NOT NULL DEFAULT magicmtn_current_organization(),
    connection_ref text NOT NULL,
    display_name text NOT NULL CHECK (length(btrim(display_name)) BETWEEN 1 AND 512),
    provider text NOT NULL CHECK (provider ~ '^[a-z][a-z0-9-]{0,63}$'),
    provider_pin jsonb NOT NULL,
    manifest jsonb NOT NULL,
    configuration jsonb NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(configuration)='object'),
    credential_mode text NOT NULL DEFAULT 'managed' CHECK (credential_mode IN ('managed','byok')),
    credential_ref text,
    runner_id uuid,
    status text NOT NULL DEFAULT 'enabled' CHECK (status IN ('enabled','disabled')),
    generation bigint NOT NULL DEFAULT 1 CHECK (generation>0),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE(organization_id,id),
    UNIQUE(organization_id,connection_ref),
    FOREIGN KEY(organization_id,runner_id) REFERENCES provider_runners(organization_id,id)
);
CREATE TABLE IF NOT EXISTS public.provider_rpc_jobs (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    organization_id uuid NOT NULL DEFAULT magicmtn_current_organization(),
    runner_id uuid NOT NULL,
    scope text NOT NULL,
    binding jsonb NOT NULL,
    operation jsonb NOT NULL,
    state text NOT NULL DEFAULT 'queued' CHECK (state IN ('queued','dispatched','completed','failed','uncertain')),
    lease_token uuid,
    result jsonb,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY(organization_id,runner_id) REFERENCES provider_runners(organization_id,id)
);
CREATE INDEX IF NOT EXISTS provider_rpc_jobs_queue ON provider_rpc_jobs(runner_id,created_at) WHERE state='queued';
CREATE TABLE IF NOT EXISTS public.provider_resource_receipts (
    organization_id uuid NOT NULL,
    scope text NOT NULL,
    kind text NOT NULL CHECK(kind IN ('sandbox','inference')),
    binding jsonb NOT NULL,
    state text NOT NULL DEFAULT 'dispatched' CHECK(state IN ('dispatched','completed','uncertain')),
    result jsonb,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY(organization_id,scope,kind)
);
CREATE TABLE IF NOT EXISTS public.provider_preferences (
    organization_id uuid NOT NULL DEFAULT magicmtn_current_organization(),
    client_kind text NOT NULL CHECK (client_kind IN ('desktop','default')),
    sandbox_connection_id uuid NOT NULL,
    PRIMARY KEY(organization_id,client_kind),
    FOREIGN KEY(organization_id,sandbox_connection_id) REFERENCES provider_connections(organization_id,id)
);
CREATE TABLE IF NOT EXISTS public.inference_resources (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    organization_id uuid NOT NULL DEFAULT magicmtn_current_organization(),
    connection_id uuid NOT NULL,
    binding jsonb NOT NULL,
    resource_name text NOT NULL,
    provider_id text,
    specification jsonb NOT NULL,
    snapshot jsonb,
    state text NOT NULL DEFAULT 'creating' CHECK (state IN ('creating','active','stopped','deleting','deleted','uncertain','failed')),
    last_error text,
    created_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY(organization_id,connection_id) REFERENCES provider_connections(organization_id,id),
    UNIQUE(organization_id,connection_id,resource_name)
);

-- Shared durable-create identity guard, used by local and Cloud assemblies.
CREATE OR REPLACE FUNCTION public.preserve_sandbox_compute_owner() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF (NEW.compute_id,NEW.compute_binding_ref,NEW.initiator_type,NEW.initiator_id,NEW.organization_id)
 IS DISTINCT FROM (OLD.compute_id,OLD.compute_binding_ref,OLD.initiator_type,OLD.initiator_id,OLD.organization_id) THEN
  RAISE EXCEPTION 'sandbox compute owner is immutable' USING ERRCODE='23514';
 END IF;
 IF NEW.provider_id IS DISTINCT FROM OLD.provider_id AND NOT (
  OLD.provider_id='pending:'||OLD.id::text AND OLD.status IN ('creating','uncertain','terminating')
  AND EXISTS(SELECT 1 FROM provider_resource_receipts r WHERE r.organization_id=OLD.organization_id
   AND r.scope='sandbox:'||OLD.id::text AND r.kind='sandbox' AND r.state='completed'
   AND r.result#>>'{value,provider_id}'=NEW.provider_id)
 ) THEN RAISE EXCEPTION 'sandbox provider identity is immutable' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END $$;

