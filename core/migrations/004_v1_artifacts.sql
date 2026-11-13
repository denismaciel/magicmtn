-- Public v1 artifacts retain the existing frozen model/compute validation.
-- The earlier validators remain for immutable local artifacts and migration history.
CREATE FUNCTION public.ck_artifact_v1(a jsonb) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE b jsonb;
BEGIN
 IF a->'contract_version' IS DISTINCT FROM '1'::jsonb THEN
  RAISE EXCEPTION 'v1 required' USING ERRCODE='23514';
 END IF;
 IF a->>'artifact_digest' IS DISTINCT FROM encode(digest(convert_to(ck_json_v1(a-'artifact_digest'),'UTF8'),'sha256'),'hex') THEN
  RAISE EXCEPTION 'artifact digest mismatch' USING ERRCODE='23514';
 END IF;
 b:=a||jsonb_build_object('contract_version',CASE WHEN a ? 'compute_manifest' THEN 3 ELSE 2 END);
 b:=b||jsonb_build_object('artifact_digest',encode(digest(convert_to(ck_json_v1(b-'artifact_digest'),'UTF8'),'sha256'),'hex'));
 IF a ? 'compute_manifest' THEN PERFORM ck_artifact_v3(b); ELSE PERFORM ck_artifact_v2(b); END IF;
END $$;

CREATE OR REPLACE FUNCTION public.validate_actual_compute() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE a JSONB; c JSONB;
BEGIN
 SELECT am.manifest INTO a FROM executions ex JOIN artifact_manifests am ON am.revision_id=ex.harness_revision_id WHERE ex.id=NEW.execution_id;
 c:=NEW.compute_context;
 IF a ? 'compute_manifest' THEN
  IF c IS NULL OR NOT coalesce(ck_closed(c,ARRAY['compute_id','backend','binding_ref','configuration','backend_version','image_identity','sandbox_id','runner_ref','completeness','provenance']) AND ck_uuid(c->'compute_id') AND ck_text(c->'runner_ref'),false)
   OR NOT coalesce(c->>'completeness' IN ('unknown','partial','complete') AND c->>'provenance' IN ('selected','observed'),false)
   OR NOT EXISTS(SELECT 1 FROM jsonb_array_elements(a->'compute_manifest') AS members(entry) WHERE entry->'compute_id'=c->'compute_id' AND entry#>'{snapshot,backend}'=c->'backend' AND entry#>'{snapshot,binding_ref}'=c->'binding_ref' AND entry#>'{snapshot,configuration}'=c->'configuration') THEN RAISE EXCEPTION 'actual compute differs from pinned revision' USING ERRCODE='23514'; END IF;
  IF c->>'provenance'='observed' AND NOT coalesce(ck_uuid(c->'sandbox_id') AND ck_text(c->'backend_version'),false) THEN RAISE EXCEPTION 'observed compute needs sandbox and backend version' USING ERRCODE='23514'; END IF;
  IF c->'sandbox_id'<>'null' AND NOT EXISTS(SELECT 1 FROM sandboxes WHERE id=(c->>'sandbox_id')::uuid AND compute_id=(c->>'compute_id')::uuid AND compute_binding_ref=c->>'binding_ref') THEN RAISE EXCEPTION 'sandbox compute owner differs' USING ERRCODE='23514'; END IF;
 ELSIF c IS NOT NULL THEN RAISE EXCEPTION 'legacy actual compute stays unknown' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END $$;

CREATE OR REPLACE FUNCTION public.validate_harness_assignment() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 IF NEW.interactive_deployment_id IS NOT NULL AND NOT EXISTS (
   SELECT 1 FROM harness_deployments d JOIN harness_revisions r ON r.id=d.revision_id
   JOIN artifact_manifests a ON a.revision_id=r.id WHERE d.id=NEW.interactive_deployment_id
   AND d.harness_id=NEW.id AND r.status='active' AND a.manifest->>'runtime_version'='interactive-v1'
   AND ((a.manifest->>'contract_version'='1' AND NOT a.manifest ? 'model_manifest') OR a.manifest#>>'{runtime_configuration,state}'='configured')
 ) THEN RAISE EXCEPTION 'assignment must identify an active configured interactive deployment' USING ERRCODE='23514'; END IF;
 IF NEW.selected_prompt_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM system_prompts WHERE id=NEW.selected_prompt_id AND harness_id=NEW.id)
 THEN RAISE EXCEPTION 'selected library prompt belongs to another harness' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END; $$;

CREATE OR REPLACE FUNCTION public.validate_model_artifact() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE revision harness_revisions%ROWTYPE;
BEGIN
    SELECT * INTO revision FROM harness_revisions WHERE id=NEW.revision_id;
    IF NEW.manifest->>'contract_version' IN ('2','3') OR (NEW.manifest->>'contract_version'='1' AND NEW.manifest ? 'model_manifest') THEN
        IF NEW.manifest->>'contract_version'='1' THEN PERFORM ck_artifact_v1(NEW.manifest); ELSIF NEW.manifest->>'contract_version'='3' THEN PERFORM ck_artifact_v3(NEW.manifest); ELSE PERFORM ck_artifact_v2(NEW.manifest); END IF;
        IF NEW.artifact_digest IS DISTINCT FROM NEW.manifest->>'artifact_digest' THEN RAISE EXCEPTION 'stored digest differs' USING ERRCODE='23514'; END IF;
        IF NEW.manifest ? 'compute_manifest' AND (revision.compute_manifest IS DISTINCT FROM NEW.manifest->'compute_manifest' OR revision.default_compute_id::text IS DISTINCT FROM NEW.manifest->>'default_compute_id') THEN RAISE EXCEPTION 'artifact compute differs' USING ERRCODE='23514'; END IF;
        IF NOT NEW.manifest ? 'compute_manifest' AND revision.compute_manifest IS NOT NULL THEN RAISE EXCEPTION 'compute membership required' USING ERRCODE='23514'; END IF;
        -- Runtime settings are frozen in surfaces for all new direct registrations.
        IF revision.surfaces ? 'runtime_configuration' AND (revision.surfaces->'runtime_configuration' IS DISTINCT FROM NEW.manifest->'runtime_configuration'
          OR revision.surfaces->'runtime_version' IS DISTINCT FROM NEW.manifest->'runtime_version'
          OR coalesce(revision.surfaces->'workflow',(SELECT to_jsonb(workflow) FROM harnesses WHERE id=revision.harness_id)) IS DISTINCT FROM NEW.manifest->'workflow') THEN
          RAISE EXCEPTION 'artifact runtime differs from frozen revision' USING ERRCODE='23514';
        END IF;
        IF revision.model_manifest IS NULL OR NEW.manifest->'model_manifest' IS DISTINCT FROM revision.model_manifest
           OR NEW.manifest->>'default_model_id' IS DISTINCT FROM revision.default_model_id::text
           OR NEW.manifest->'model_configuration' IS DISTINCT FROM revision.model_configuration
           OR NEW.manifest->>'system_prompt' IS DISTINCT FROM revision.system_prompt
           OR NEW.manifest->'prompt_provenance' IS DISTINCT FROM revision.prompt_provenance
           OR NEW.manifest->'tool_manifest' IS DISTINCT FROM revision.tool_manifest
           OR NEW.manifest->>'harness_id' IS DISTINCT FROM revision.harness_id::text
           OR NEW.manifest->>'harness_revision_id' IS DISTINCT FROM revision.id::text THEN
            RAISE EXCEPTION 'artifact differs from frozen revision' USING ERRCODE='23514';
        END IF;
    ELSIF revision.model_manifest IS NOT NULL THEN
        RAISE EXCEPTION 'registered models require a frozen artifact' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;

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
 IF a ? 'compute_manifest' THEN
  IF NEW.selected_compute_id IS NULL OR NOT EXISTS(SELECT 1 FROM jsonb_array_elements(a->'compute_manifest') AS members(entry) WHERE entry->>'compute_id'=NEW.selected_compute_id::text AND entry->'snapshot'=NEW.compute_snapshot) OR NOT EXISTS(SELECT 1 FROM tenant_compute WHERE id=NEW.selected_compute_id AND status='enabled' AND retired_at IS NULL) THEN RAISE EXCEPTION 'invalid session compute selection' USING ERRCODE='23514'; END IF;
 ELSIF NEW.selected_compute_id IS NOT NULL THEN RAISE EXCEPTION 'legacy sessions cannot fabricate compute IDs' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END $$;

CREATE OR REPLACE FUNCTION public.validate_session_model() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE a JSONB;
BEGIN
 -- The session_compute trigger freezes every identity and
 -- configuration column on UPDATE, including these additive model fields.
 IF TG_OP='UPDATE' THEN RETURN NEW; END IF;
 SELECT manifest INTO a FROM artifact_manifests WHERE revision_id=NEW.revision_id;
 IF a ? 'model_manifest' THEN
  IF NEW.selected_model_id IS NULL OR NOT EXISTS(
   SELECT 1 FROM jsonb_array_elements(a->'model_manifest') members(entry)
   WHERE entry->>'model_id'=NEW.selected_model_id::text AND entry->'snapshot'=NEW.model_snapshot
  ) OR NOT EXISTS(SELECT 1 FROM tenant_models WHERE id=NEW.selected_model_id AND status='enabled')
   OR NOT coalesce(NEW.model_snapshot#>>'{capabilities,completeness}'<>'unknown'
     AND NEW.model_snapshot#>'{capabilities,tasks}' ? 'chat'
     AND NEW.model_snapshot#>'{capabilities,tool_calls}'='true'::jsonb,false)
  THEN RAISE EXCEPTION 'invalid session model selection' USING ERRCODE='23514'; END IF;
 ELSIF NEW.selected_model_id IS NOT NULL THEN
  RAISE EXCEPTION 'legacy sessions cannot fabricate model IDs' USING ERRCODE='23514';
 END IF;
 RETURN NEW;
END $$;
