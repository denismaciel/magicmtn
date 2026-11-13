-- Bind every provider resource to this installation's one persistent scope.
DO $$
DECLARE relation text;
BEGIN
    FOREACH relation IN ARRAY ARRAY['provider_runners','provider_connections','provider_rpc_jobs','provider_resource_receipts','provider_preferences','inference_resources'] LOOP
        EXECUTE format('ALTER TABLE %I ADD FOREIGN KEY(organization_id) REFERENCES local_workspace(id)', relation);
    END LOOP;
END $$;
