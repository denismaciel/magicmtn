-- Allow initial provider pin/dispatch while preserving session identity guards.
CREATE OR REPLACE FUNCTION public.validate_session_compute() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE a JSONB;
BEGIN
 IF TG_OP='UPDATE' THEN
  IF OLD.provider_create_started AND NOT NEW.provider_create_started THEN RAISE EXCEPTION 'provider create dispatch is immutable' USING ERRCODE='23514'; END IF;
  IF (to_jsonb(NEW)-ARRAY['display_name','sandbox_id','state','last_sequence','updated_at','provider_bindings','provider_create_started']) IS DISTINCT FROM (to_jsonb(OLD)-ARRAY['display_name','sandbox_id','state','last_sequence','updated_at','provider_bindings','provider_create_started']) OR (OLD.sandbox_id IS NOT NULL AND NEW.sandbox_id IS DISTINCT FROM OLD.sandbox_id) THEN RAISE EXCEPTION 'session identity/compute pin is immutable' USING ERRCODE='23514'; END IF;
  RETURN NEW;
 END IF;
 SELECT manifest INTO a FROM artifact_manifests WHERE revision_id=NEW.revision_id;
 IF a->>'contract_version'='3' THEN
  IF NEW.selected_compute_id IS NULL OR NOT EXISTS(SELECT 1 FROM jsonb_array_elements(a->'compute_manifest') AS members(entry) WHERE entry->>'compute_id'=NEW.selected_compute_id::text AND entry->'snapshot'=NEW.compute_snapshot) OR NOT EXISTS(SELECT 1 FROM tenant_compute WHERE id=NEW.selected_compute_id AND status='enabled' AND retired_at IS NULL) THEN RAISE EXCEPTION 'invalid session compute selection' USING ERRCODE='23514'; END IF;
 ELSIF NEW.selected_compute_id IS NOT NULL THEN RAISE EXCEPTION 'legacy sessions cannot fabricate compute IDs' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END $$;

