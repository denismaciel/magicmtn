DO $$
DECLARE relation text;
BEGIN
    FOREACH relation IN ARRAY ARRAY['sandboxes','sandbox_lifecycle_jobs','chat_sessions','chat_turns','artifact_manifests','tenant_compute','tenant_models'] LOOP
        EXECUTE format('ALTER TABLE %I ADD COLUMN organization_id uuid NOT NULL DEFAULT magicmtn_current_organization() REFERENCES local_workspace(id)', relation);
    END LOOP;
END $$;
ALTER TABLE sandboxes ADD COLUMN attributed_user_id uuid;

CREATE TABLE organization_providers (
    organization_id uuid NOT NULL DEFAULT magicmtn_current_organization() REFERENCES local_workspace(id),
    provider text NOT NULL,
    version text NOT NULL,
    status text NOT NULL DEFAULT 'enabled',
    PRIMARY KEY(organization_id, provider)
);
