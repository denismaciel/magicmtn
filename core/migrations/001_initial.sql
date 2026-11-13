SET LOCAL check_function_bodies = false;
-- Fresh MagicMtn schema; independent of retained source migration history.


CREATE EXTENSION IF NOT EXISTS pgcrypto WITH SCHEMA public;

CREATE FUNCTION public.ck_artifact_v2(a jsonb) RETURNS void
    LANGUAGE plpgsql IMMUTABLE
    AS $_$
DECLARE r jsonb; t jsonb; entry jsonb; selected jsonb; n jsonb; previous uuid; model_uuid uuid;
BEGIN
 IF NOT coalesce(ck_closed(a,ARRAY['contract_version','canonicalization_version','harness_id','harness_revision_id','workflow','runtime_version','system_prompt','prompt_provenance','model_manifest','default_model_id','model_configuration','tool_manifest','runtime_configuration','artifact_digest']),false)
 OR a->'contract_version' IS DISTINCT FROM '2' OR a->>'canonicalization_version' IS DISTINCT FROM 'ck-json-v1'
 OR NOT coalesce(ck_uuid(a->'harness_id') AND ck_uuid(a->'harness_revision_id') AND ck_uuid(a->'default_model_id') AND ck_text(a->'workflow') AND ck_text(a->'system_prompt'),false)
 OR jsonb_typeof(a->'model_manifest') IS DISTINCT FROM 'array' OR jsonb_array_length(a->'model_manifest')=0
 OR jsonb_typeof(a->'runtime_configuration') IS DISTINCT FROM 'object' OR jsonb_typeof(a->'tool_manifest') IS DISTINCT FROM 'array' THEN
   RAISE EXCEPTION 'invalid v2 artifact structure' USING ERRCODE='23514';
 END IF;
 PERFORM ck_prompt(a->'prompt_provenance');
 FOR entry IN SELECT value FROM jsonb_array_elements(a->'model_manifest') LOOP
   IF NOT coalesce(ck_closed(entry,ARRAY['model_id','snapshot']) AND ck_uuid(entry->'model_id'),false) THEN RAISE EXCEPTION 'invalid manifest entry' USING ERRCODE='23514'; END IF;
   model_uuid:=(entry->>'model_id')::uuid;
   IF previous IS NOT NULL AND previous>=model_uuid THEN RAISE EXCEPTION 'unique sorted model IDs required' USING ERRCODE='23514'; END IF;
   previous:=model_uuid;
   PERFORM ck_model_snapshot(entry->'snapshot');
   IF entry->'model_id'=a->'default_model_id' THEN selected:=entry->'snapshot'; END IF;
 END LOOP;
 IF selected IS NULL OR a->'model_configuration' IS DISTINCT FROM selected->'model_configuration' THEN RAISE EXCEPTION 'invalid default projection' USING ERRCODE='23514'; END IF;
 IF a->>'artifact_digest' IS DISTINCT FROM encode(sha256(convert_to(ck_json_v1(a-'artifact_digest'),'UTF8')),'hex') THEN
   RAISE EXCEPTION 'artifact digest mismatch' USING ERRCODE='23514';
 END IF;
 r:=a->'runtime_configuration';
 CASE a->>'runtime_version'
 WHEN 'unconfigured' THEN
   IF r<>'{"state":"unconfigured"}' OR length(a->>'system_prompt')>100000 THEN RAISE EXCEPTION 'invalid draft runtime' USING ERRCODE='23514'; END IF;
 WHEN 'interactive-v1' THEN
   IF a->>'workflow'<>'interactive' OR octet_length(a->>'system_prompt')>4096 THEN RAISE EXCEPTION 'invalid interactive workflow/prompt' USING ERRCODE='23514'; END IF;
   IF r->>'state'='unconfigured' THEN
     IF r<>'{"state":"unconfigured"}' THEN RAISE EXCEPTION 'invalid draft runtime' USING ERRCODE='23514'; END IF;
   ELSE
     IF NOT coalesce(ck_closed(r,ARRAY['state','sandbox_image','limits']) AND ck_text(r->'sandbox_image'),false) OR r->>'state' IS DISTINCT FROM 'configured'
     OR r->>'sandbox_image' ~ '[[:space:]]' OR NOT coalesce(ck_closed(r->'limits',ARRAY['max_model_calls','max_tool_calls','max_turn_seconds','command_timeout_ms','max_output_bytes','max_context_messages']),false) THEN RAISE EXCEPTION 'invalid interactive runtime' USING ERRCODE='23514'; END IF;
     FOR n IN SELECT value FROM jsonb_each(r->'limits') LOOP
       IF jsonb_typeof(n) IS DISTINCT FROM 'number' OR n::text !~ '^[1-9][0-9]*$' OR n::text::numeric>9007199254740991 THEN RAISE EXCEPTION 'invalid limit' USING ERRCODE='23514'; END IF;
     END LOOP;
     IF jsonb_array_length(a->'tool_manifest')<>1 THEN RAISE EXCEPTION 'run_command required' USING ERRCODE='23514'; END IF;
     t:=a->'tool_manifest'->0;
     IF NOT coalesce(ck_closed(t,ARRAY['type','function']) AND ck_closed(t->'function',ARRAY['name','description','parameters']) AND ck_text(t->'function'->'description'),false)
     OR t->>'type' IS DISTINCT FROM 'function' OR t->'function'->>'name' IS DISTINCT FROM 'run_command' OR jsonb_typeof(t->'function'->'parameters') IS DISTINCT FROM 'object' THEN RAISE EXCEPTION 'invalid interactive tool' USING ERRCODE='23514'; END IF;
     IF selected->'model_configuration'->>'provider' IS DISTINCT FROM 'ollama' OR selected->'connection_ref'='null'
     OR NOT (selected->'capabilities'->'tasks' ? 'chat') OR selected->'capabilities'->'tool_calls' IS DISTINCT FROM 'true' THEN RAISE EXCEPTION 'incompatible default model' USING ERRCODE='23514'; END IF;
   END IF;
 ELSE RAISE EXCEPTION 'unknown runtime' USING ERRCODE='23514';
 END CASE;
END $_$;

CREATE FUNCTION public.ck_artifact_v3(a jsonb) RETURNS void
    LANGUAGE plpgsql
    AS $$
DECLARE b JSONB; selected JSONB;
BEGIN
 PERFORM ck_compute_manifest(a->'compute_manifest',(a->>'default_compute_id')::uuid);
 IF a->'contract_version' IS DISTINCT FROM '3' THEN RAISE EXCEPTION 'v3 required' USING ERRCODE='23514'; END IF;
 b:=(a-ARRAY['compute_manifest','default_compute_id'])||'{"contract_version":2}';
 b:=b||jsonb_build_object('artifact_digest',encode(sha256(convert_to(ck_json_v1(b-'artifact_digest'),'UTF8')),'hex'));
 PERFORM ck_artifact_v2(b);
 IF a->>'artifact_digest' IS DISTINCT FROM encode(sha256(convert_to(ck_json_v1(a-'artifact_digest'),'UTF8')),'hex') THEN RAISE EXCEPTION 'v3 digest mismatch' USING ERRCODE='23514'; END IF;
 IF a->>'runtime_version'='interactive-v1' AND a#>>'{runtime_configuration,state}'='configured' THEN
  SELECT e->'snapshot' INTO selected FROM jsonb_array_elements(a->'compute_manifest') e WHERE e->>'compute_id'=a->>'default_compute_id';
  IF selected->'configuration' ? 'image' AND a#>'{runtime_configuration,sandbox_image}' IS DISTINCT FROM selected#>'{configuration,image}' THEN RAISE EXCEPTION 'conflicting image' USING ERRCODE='23514'; END IF;
 END IF;
END $$;

CREATE FUNCTION public.ck_closed(v jsonb, keys text[]) RETURNS boolean
    LANGUAGE sql IMMUTABLE
    AS $$
 SELECT jsonb_typeof(v)='object' AND v ?& keys
 AND (SELECT count(*) FROM jsonb_object_keys(v))=cardinality(keys)
$$;

CREATE FUNCTION public.ck_compute_manifest(m jsonb, d uuid) RETURNS void
    LANGUAGE plpgsql
    AS $$
DECLARE projected JSONB:='[]'; e JSONB;
BEGIN
 IF jsonb_typeof(m) IS DISTINCT FROM 'array' THEN RAISE EXCEPTION 'compute array required' USING ERRCODE='23514'; END IF;
 FOR e IN SELECT value FROM jsonb_array_elements(m) LOOP
  -- Empty backend configuration is the new representation. Legacy image snapshots
  -- keep their original validation and digests. The temporary image is validation
  -- only, never written into an artifact or revision.
  IF e#>'{snapshot,configuration}'='{}'::jsonb THEN
   e:=jsonb_set(e,'{snapshot,configuration}','{"image":"legacy-validation-placeholder"}');
  END IF;
  projected:=projected||jsonb_build_array(e);
 END LOOP;
 PERFORM ck_compute_manifest_legacy(projected,d);
 IF EXISTS(SELECT 1 FROM jsonb_array_elements(m) x WHERE x#>'{snapshot,configuration}'='{}'::jsonb)
  AND (SELECT count(*) FROM jsonb_array_elements(m))<>(SELECT count(DISTINCT x#>>'{snapshot,backend}') FROM jsonb_array_elements(m) x)
 THEN RAISE EXCEPTION 'one member per backend implementation' USING ERRCODE='23514'; END IF;
END $$;

CREATE FUNCTION public.ck_compute_manifest_legacy(m jsonb, d uuid) RETURNS void
    LANGUAGE plpgsql
    AS $_$
DECLARE e JSONB; previous UUID; cid UUID; s JSONB;
BEGIN
 IF m IS NULL OR jsonb_typeof(m)<>'array' OR jsonb_array_length(m)=0 OR d IS NULL THEN RAISE EXCEPTION 'compute required' USING ERRCODE='23514'; END IF;
 FOR e IN SELECT value FROM jsonb_array_elements(m) LOOP
  IF NOT coalesce(ck_closed(e,ARRAY['compute_id','snapshot']) AND ck_uuid(e->'compute_id'),false) THEN RAISE EXCEPTION 'invalid compute entry' USING ERRCODE='23514'; END IF;
  cid:=(e->>'compute_id')::uuid;s:=e->'snapshot';
  IF previous IS NOT NULL AND cid<=previous THEN RAISE EXCEPTION 'sorted unique compute required' USING ERRCODE='23514'; END IF;previous:=cid;
  IF NOT coalesce(ck_closed(s,ARRAY['display_name','description','backend','binding_ref','configuration','capabilities']) AND ck_text(s->'display_name') AND ck_closed(s->'configuration',ARRAY['image']) AND ck_text(s->'configuration'->'image') AND ck_closed(s->'capabilities',ARRAY['completeness','operations']),false)
   OR s->>'backend' IS DISTINCT FROM 'apple-container' OR s->>'binding_ref' !~ '^binding:[A-Za-z0-9_.:-]+$'
   OR s#>>'{configuration,image}' !~ '^[A-Za-z0-9/_.:@-]+$' OR jsonb_typeof(s#>'{capabilities,operations}') IS DISTINCT FROM 'array'
   OR NOT coalesce(s#>>'{capabilities,completeness}' IN ('unknown','partial','complete'),false)
   OR jsonb_typeof(s->'description') IS DISTINCT FROM 'string' OR octet_length(s->>'description')>8000 OR octet_length(s->>'display_name')>512 OR octet_length(s->>'binding_ref')>200
   OR octet_length(s#>>'{configuration,image}')>512 OR position('://' in s#>>'{configuration,image}')>0 THEN RAISE EXCEPTION 'invalid compute snapshot' USING ERRCODE='23514'; END IF;
  IF EXISTS(SELECT 1 FROM jsonb_array_elements_text(s#>'{capabilities,operations}') op WHERE op NOT IN ('create','exec_bounded','terminate')) OR (s#>>'{capabilities,completeness}'='unknown' AND jsonb_array_length(s#>'{capabilities,operations}')>0) THEN RAISE EXCEPTION 'invalid compute capabilities' USING ERRCODE='23514'; END IF;
  IF jsonb_array_length(s#>'{capabilities,operations}')<>(SELECT count(DISTINCT value) FROM jsonb_array_elements_text(s#>'{capabilities,operations}')) THEN RAISE EXCEPTION 'duplicate operations' USING ERRCODE='23514'; END IF;
  IF NOT EXISTS(SELECT 1 FROM tenant_compute WHERE id=cid) THEN RAISE EXCEPTION 'unknown compute' USING ERRCODE='23514'; END IF;
 END LOOP;
 IF NOT EXISTS(SELECT 1 FROM jsonb_array_elements(m) AS members(entry) WHERE entry->>'compute_id'=d::text) THEN RAISE EXCEPTION 'default outside compute' USING ERRCODE='23514'; END IF;
END $_$;

CREATE FUNCTION public.ck_json_v1(v jsonb) RETURNS text
    LANGUAGE plpgsql IMMUTABLE
    AS $_$
DECLARE result text; item record;
BEGIN
 CASE jsonb_typeof(v)
 WHEN 'object' THEN
   FOR item IN SELECT key,value FROM jsonb_each(v) LOOP
     IF octet_length(item.key)<>length(item.key) THEN RAISE EXCEPTION 'canonical keys must be ASCII' USING ERRCODE='23514'; END IF;
   END LOOP;
   SELECT '{'||coalesce(string_agg(to_jsonb(key)::text||':'||ck_json_v1(value),',' ORDER BY key COLLATE "C"),'')||'}' INTO result FROM jsonb_each(v);
 WHEN 'array' THEN
   SELECT '['||coalesce(string_agg(ck_json_v1(value),',' ORDER BY ord),'')||']' INTO result FROM jsonb_array_elements(v) WITH ORDINALITY a(value,ord);
 WHEN 'number' THEN
   IF v::text !~ '^-?(0|[1-9][0-9]*)$' OR abs(v::text::numeric)>9007199254740991 THEN
     RAISE EXCEPTION 'canonical numbers must be safe integers' USING ERRCODE='23514';
   END IF;
   result:=v::text;
 ELSE result:=v::text;
 END CASE;
 RETURN result;
END $_$;

CREATE FUNCTION public.ck_model_snapshot(s jsonb) RETURNS void
    LANGUAGE plpgsql IMMUTABLE
    AS $_$
DECLARE m jsonb; c jsonb; task jsonb; previous text;
BEGIN
 IF NOT coalesce(ck_closed(s,ARRAY['source','source_model_id','display_name','description','task_type','model_configuration','connection_ref','capabilities','provenance']),false)
 OR NOT coalesce(ck_text(s->'source') AND ck_text(s->'source_model_id') AND ck_text(s->'display_name') AND ck_text(s->'task_type'),false)
 OR jsonb_typeof(s->'description') IS DISTINCT FROM 'string' OR NOT coalesce((s->>'provenance') IN ('registered','legacy_migration'),false) THEN
   RAISE EXCEPTION 'invalid complete model snapshot' USING ERRCODE='23514';
 END IF;
 m:=s->'model_configuration'; c:=s->'capabilities';
 IF NOT coalesce(ck_closed(m,ARRAY['model','provider','version','parameters']) AND ck_text(m->'model'),false)
 OR NOT coalesce((m->'provider'='null' OR ck_text(m->'provider')) AND (m->'version'='null' OR ck_text(m->'version')),false)
 OR jsonb_typeof(m->'parameters') IS DISTINCT FROM 'object' THEN
   RAISE EXCEPTION 'invalid model configuration' USING ERRCODE='23514';
 END IF;
 PERFORM ck_nonsecret(m->'parameters'); PERFORM ck_json_v1(s);
 IF s->'connection_ref'<>'null' AND (s->>'connection_ref') !~ '^connection:[A-Za-z0-9_.:-]{1,189}$' THEN
   RAISE EXCEPTION 'opaque connection reference required' USING ERRCODE='23514';
 END IF;
 IF jsonb_typeof(s->'connection_ref') NOT IN ('string','null') OR NOT coalesce(ck_closed(c,ARRAY['completeness','tasks','tool_calls']),false)
 OR NOT coalesce((c->>'completeness') IN ('unknown','partial','complete'),false) OR jsonb_typeof(c->'tasks') IS DISTINCT FROM 'array'
 OR jsonb_typeof(c->'tool_calls') NOT IN ('boolean','null') THEN
   RAISE EXCEPTION 'invalid capabilities' USING ERRCODE='23514';
 END IF;
 FOR task IN SELECT value FROM jsonb_array_elements(c->'tasks') LOOP
   IF NOT coalesce(ck_text(task),false) OR (previous IS NOT NULL AND previous COLLATE "C">=(task #>> '{}') COLLATE "C") THEN
     RAISE EXCEPTION 'unique sorted capability tasks required' USING ERRCODE='23514';
   END IF;
   previous:=task #>> '{}';
 END LOOP;
 IF c->>'completeness'='unknown' AND (c->'tasks'<>'[]' OR c->'tool_calls'<>'null') THEN
   RAISE EXCEPTION 'unknown capabilities cannot assert support' USING ERRCODE='23514';
 END IF;
END $_$;

CREATE FUNCTION public.ck_nonsecret(v jsonb) RETURNS void
    LANGUAGE plpgsql IMMUTABLE
    AS $$
DECLARE item record; child jsonb;
BEGIN
 IF jsonb_typeof(v)='object' THEN
   FOR item IN SELECT key,value FROM jsonb_each(v) LOOP
     IF regexp_replace(lower(item.key),'[^a-z]','','g')=ANY(ARRAY['token','accesstoken','apikey','password','secret','authorization','credentials','credential','headers','endpoint','url','baseurl']) THEN
       RAISE EXCEPTION 'secret/transport parameter forbidden' USING ERRCODE='23514';
     END IF;
     PERFORM ck_nonsecret(item.value);
   END LOOP;
 ELSIF jsonb_typeof(v)='array' THEN
   FOR child IN SELECT value FROM jsonb_array_elements(v) LOOP PERFORM ck_nonsecret(child); END LOOP;
 ELSIF jsonb_typeof(v)='string' AND position('://' IN v #>> '{}')>0 THEN
   RAISE EXCEPTION 'transport URL forbidden in parameters' USING ERRCODE='23514';
 END IF;
END $$;

CREATE FUNCTION public.ck_prompt(p jsonb) RETURNS void
    LANGUAGE plpgsql IMMUTABLE
    AS $$
BEGIN
 IF NOT coalesce(ck_closed(p,ARRAY['kind','library_prompt_id']),false) OR NOT coalesce(p->>'kind' IN ('inline','library','legacy_migration'),false)
 OR NOT coalesce(p->'library_prompt_id'='null' OR ck_uuid(p->'library_prompt_id'),false)
 OR (p->>'kind'='inline' AND p->'library_prompt_id'<>'null') OR (p->>'kind'='library' AND p->'library_prompt_id'='null') THEN
   RAISE EXCEPTION 'invalid prompt provenance' USING ERRCODE='23514';
 END IF;
END $$;

CREATE FUNCTION public.ck_text(v jsonb) RETURNS boolean
    LANGUAGE sql IMMUTABLE
    AS $_$
 SELECT jsonb_typeof(v)='string' AND (v #>> '{}') !~ '^[[:space:]]*$'
$_$;

CREATE FUNCTION public.ck_uuid(v jsonb) RETURNS boolean
    LANGUAGE plpgsql IMMUTABLE
    AS $$
BEGIN
 RETURN jsonb_typeof(v)='string' AND (v #>> '{}')::uuid <> '00000000-0000-0000-0000-000000000000'::uuid
 AND (v #>> '{}')::uuid::text=v #>> '{}';
EXCEPTION WHEN invalid_text_representation THEN RETURN false;
END $$;

CREATE FUNCTION public.preserve_sandbox_compute_owner() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 IF (NEW.compute_id,NEW.compute_binding_ref,NEW.provider_id,NEW.initiator_type,NEW.initiator_id) IS DISTINCT FROM (OLD.compute_id,OLD.compute_binding_ref,OLD.provider_id,OLD.initiator_type,OLD.initiator_id) THEN RAISE EXCEPTION 'sandbox compute owner is immutable' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END $$;

CREATE FUNCTION public.protect_artifact_record() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    RAISE EXCEPTION 'immutable artifact; append a new revision' USING ERRCODE='23514';
END;
$$;

CREATE FUNCTION public.protect_execution_record() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    RAISE EXCEPTION '% records are immutable; append a new context or execution', TG_TABLE_NAME
        USING ERRCODE = '23514';
END;
$$;

CREATE FUNCTION public.protect_harness_revision() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'harness revisions cannot be deleted' USING ERRCODE = '23514';
    END IF;
    IF (to_jsonb(NEW) - ARRAY['status', 'activated_at', 'retired_at']) IS DISTINCT FROM
       (to_jsonb(OLD) - ARRAY['status', 'activated_at', 'retired_at']) THEN
        RAISE EXCEPTION 'harness revision configuration is immutable; create a new revision'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION public.protect_registered_model() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM harness_revisions r, jsonb_array_elements(r.model_manifest) e WHERE e->>'model_id'=OLD.id::text) THEN
        RAISE EXCEPTION 'referenced model registrations must be disabled rather than deleted' USING ERRCODE='23514';
    END IF;
    RETURN OLD;
END;
$$;

CREATE FUNCTION public.require_existing_parent_revision() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF NEW.parent_revision_id IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM harness_revisions WHERE id = NEW.parent_revision_id
    ) THEN
        RAISE EXCEPTION 'parent revision must exist before its child is created'
            USING ERRCODE = '23503';
    END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION public.validate_actual_compute() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE a JSONB; c JSONB;
BEGIN
 SELECT am.manifest INTO a FROM executions ex JOIN artifact_manifests am ON am.revision_id=ex.harness_revision_id WHERE ex.id=NEW.execution_id;
 c:=NEW.compute_context;
 IF a->>'contract_version'='3' THEN
  IF c IS NULL OR NOT coalesce(ck_closed(c,ARRAY['compute_id','backend','binding_ref','configuration','backend_version','image_identity','sandbox_id','runner_ref','completeness','provenance']) AND ck_uuid(c->'compute_id') AND ck_text(c->'runner_ref'),false)
   OR NOT coalesce(c->>'completeness' IN ('unknown','partial','complete') AND c->>'provenance' IN ('selected','observed'),false)
   OR NOT EXISTS(SELECT 1 FROM jsonb_array_elements(a->'compute_manifest') AS members(entry) WHERE entry->'compute_id'=c->'compute_id' AND entry#>'{snapshot,backend}'=c->'backend' AND entry#>'{snapshot,binding_ref}'=c->'binding_ref' AND entry#>'{snapshot,configuration}'=c->'configuration') THEN RAISE EXCEPTION 'actual compute differs from pinned revision' USING ERRCODE='23514'; END IF;
  IF c->>'provenance'='observed' AND NOT coalesce(ck_uuid(c->'sandbox_id') AND ck_text(c->'backend_version'),false) THEN RAISE EXCEPTION 'observed compute needs sandbox and backend version' USING ERRCODE='23514'; END IF;
  IF c->'sandbox_id'<>'null' AND NOT EXISTS(SELECT 1 FROM sandboxes WHERE id=(c->>'sandbox_id')::uuid AND compute_id=(c->>'compute_id')::uuid AND compute_binding_ref=c->>'binding_ref') THEN RAISE EXCEPTION 'sandbox compute owner differs' USING ERRCODE='23514'; END IF;
 ELSIF c IS NOT NULL THEN RAISE EXCEPTION 'legacy actual compute stays unknown' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END $$;

CREATE FUNCTION public.validate_harness_assignment() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 IF NEW.interactive_deployment_id IS NOT NULL AND NOT EXISTS (
   SELECT 1 FROM harness_deployments d JOIN harness_revisions r ON r.id=d.revision_id
   JOIN artifact_manifests a ON a.revision_id=r.id WHERE d.id=NEW.interactive_deployment_id
   AND d.harness_id=NEW.id AND r.status='active' AND a.manifest->>'runtime_version'='interactive-v1'
   AND (a.manifest->>'contract_version'='1' OR a.manifest#>>'{runtime_configuration,state}'='configured')
 ) THEN RAISE EXCEPTION 'assignment must identify an active configured interactive deployment' USING ERRCODE='23514'; END IF;
 IF NEW.selected_prompt_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM system_prompts WHERE id=NEW.selected_prompt_id AND harness_id=NEW.id)
 THEN RAISE EXCEPTION 'selected library prompt belongs to another harness' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END; $$;

CREATE FUNCTION public.validate_model_artifact() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE revision harness_revisions%ROWTYPE;
BEGIN
    SELECT * INTO revision FROM harness_revisions WHERE id=NEW.revision_id;
    IF NEW.manifest->>'contract_version' IN ('2','3') THEN
        IF NEW.manifest->>'contract_version'='3' THEN PERFORM ck_artifact_v3(NEW.manifest); ELSE PERFORM ck_artifact_v2(NEW.manifest); END IF;
        IF NEW.artifact_digest IS DISTINCT FROM NEW.manifest->>'artifact_digest' THEN RAISE EXCEPTION 'stored digest differs' USING ERRCODE='23514'; END IF;
        IF NEW.manifest->>'contract_version'='3' AND (revision.compute_manifest IS DISTINCT FROM NEW.manifest->'compute_manifest' OR revision.default_compute_id::text IS DISTINCT FROM NEW.manifest->>'default_compute_id') THEN RAISE EXCEPTION 'artifact compute differs' USING ERRCODE='23514'; END IF;
        IF NEW.manifest->>'contract_version'='2' AND revision.compute_manifest IS NOT NULL THEN RAISE EXCEPTION 'compute requires v3' USING ERRCODE='23514'; END IF;
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
        RAISE EXCEPTION 'multimodel revisions require v2 artifacts' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION public.validate_revision_compute() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
 IF NEW.compute_manifest IS NOT NULL THEN
  PERFORM ck_compute_manifest(NEW.compute_manifest,NEW.default_compute_id);
  IF TG_OP='INSERT' AND EXISTS(
   SELECT 1 FROM jsonb_array_elements(NEW.compute_manifest) members(entry)
   JOIN tenant_compute c ON c.id=(entry->>'compute_id')::uuid
   WHERE (c.status<>'enabled' OR c.retired_at IS NOT NULL) AND NOT EXISTS(
    SELECT 1 FROM harness_revisions parent CROSS JOIN LATERAL jsonb_array_elements(parent.compute_manifest) inherited(entry)
    WHERE parent.id=NEW.parent_revision_id AND inherited.entry=members.entry
   )
  ) THEN RAISE EXCEPTION 'new or refreshed disabled compute selection' USING ERRCODE='23514'; END IF;
 ELSIF TG_OP='INSERT' AND EXISTS(SELECT 1 FROM harness_revisions WHERE id=NEW.parent_revision_id AND compute_manifest IS NOT NULL) THEN RAISE EXCEPTION 'compute cannot be dropped' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END $$;

CREATE FUNCTION public.validate_revision_models() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE entry JSONB; model_uuid UUID; previous_uuid UUID; configured JSONB; default_snapshot JSONB;
BEGIN
    -- Explicit transition boundary: existing v1 writers retain their v1 representation.
    IF NEW.model_manifest IS NULL THEN
        IF EXISTS (SELECT 1 FROM harnesses WHERE id=NEW.harness_id AND definition_revision_id IS NOT NULL)
           AND NOT EXISTS (SELECT 1 FROM harness_revisions WHERE id=NEW.id) THEN
            RAISE EXCEPTION 'configured harnesses require model-set-aware authoring' USING ERRCODE='23514';
        END IF;
        RETURN NEW;
    END IF;
    PERFORM ck_prompt(NEW.prompt_provenance);
    IF NOT coalesce(ck_text(to_jsonb(NEW.system_prompt)),false) OR NOT (NEW.surfaces ?& ARRAY['runtime_version','runtime_configuration']) THEN
        RAISE EXCEPTION 'complete prompt/runtime snapshot required' USING ERRCODE='23514';
    END IF;
    IF length(NEW.system_prompt)>100000 THEN RAISE EXCEPTION 'prompt too large' USING ERRCODE='23514'; END IF;
    IF NEW.prompt_provenance->'library_prompt_id'<>'null' AND NOT EXISTS (
        SELECT 1 FROM system_prompts WHERE id=(NEW.prompt_provenance->>'library_prompt_id')::uuid AND harness_id=NEW.harness_id
    ) THEN RAISE EXCEPTION 'prompt library ownership mismatch' USING ERRCODE='23514'; END IF;
    FOR entry IN SELECT value FROM jsonb_array_elements(NEW.model_manifest) LOOP
        IF jsonb_typeof(entry) IS DISTINCT FROM 'object' OR jsonb_typeof(entry->'model_id') IS DISTINCT FROM 'string'
           OR jsonb_typeof(entry->'snapshot') IS DISTINCT FROM 'object' THEN
            RAISE EXCEPTION 'invalid model manifest entry' USING ERRCODE='23514';
        END IF;
        IF NOT coalesce(ck_closed(entry,ARRAY['model_id','snapshot']),false) THEN
            RAISE EXCEPTION 'invalid manifest entry fields' USING ERRCODE='23514';
        END IF;
        PERFORM ck_model_snapshot(entry->'snapshot');
        model_uuid := (entry->>'model_id')::uuid;
        IF model_uuid::text <> entry->>'model_id' OR (previous_uuid IS NOT NULL AND model_uuid <= previous_uuid) THEN
            RAISE EXCEPTION 'model manifest must contain unique canonical UUIDs in sorted order' USING ERRCODE='23514';
        END IF;
        PERFORM 1 FROM tenant_models WHERE id=model_uuid AND (status='enabled' OR EXISTS(SELECT 1 FROM harness_revisions WHERE id=NEW.id AND model_manifest=NEW.model_manifest)) FOR SHARE;
        IF NOT FOUND THEN RAISE EXCEPTION 'unknown or disabled model registration' USING ERRCODE='23514'; END IF;
        configured := entry->'snapshot'->'model_configuration';
        IF jsonb_typeof(configured) IS DISTINCT FROM 'object' OR jsonb_typeof(configured->'model') IS DISTINCT FROM 'string'
           OR length(btrim(configured->>'model'))=0 OR NOT (configured ?& ARRAY['model','provider','version','parameters'])
           OR jsonb_typeof(configured->'parameters') IS DISTINCT FROM 'object'
           OR NOT ((entry->'snapshot') ?& ARRAY['source','source_model_id','display_name','description',
                                              'task_type','connection_ref','capabilities','provenance']) THEN
            RAISE EXCEPTION 'incomplete model snapshot' USING ERRCODE='23514';
        END IF;
        IF model_uuid=NEW.default_model_id THEN default_snapshot:=configured; END IF;
        previous_uuid:=model_uuid;
    END LOOP;
    IF default_snapshot IS NULL OR NEW.model_configuration IS DISTINCT FROM default_snapshot
       OR NEW.model IS DISTINCT FROM (default_snapshot->>'model') THEN
        RAISE EXCEPTION 'default model must belong to the manifest and match compatibility projections' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION public.validate_session_compute() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE a JSONB;
BEGIN
 IF TG_OP='UPDATE' THEN
  IF (to_jsonb(NEW)-ARRAY['display_name','sandbox_id','state','last_sequence','updated_at']) IS DISTINCT FROM (to_jsonb(OLD)-ARRAY['display_name','sandbox_id','state','last_sequence','updated_at']) OR (OLD.sandbox_id IS NOT NULL AND NEW.sandbox_id IS DISTINCT FROM OLD.sandbox_id) THEN RAISE EXCEPTION 'session identity/compute pin is immutable' USING ERRCODE='23514'; END IF;
  RETURN NEW;
 END IF;
 SELECT manifest INTO a FROM artifact_manifests WHERE revision_id=NEW.revision_id;
 IF a->>'contract_version'='3' THEN
  IF NEW.selected_compute_id IS NULL OR NOT EXISTS(SELECT 1 FROM jsonb_array_elements(a->'compute_manifest') AS members(entry) WHERE entry->>'compute_id'=NEW.selected_compute_id::text AND entry->'snapshot'=NEW.compute_snapshot) OR NOT EXISTS(SELECT 1 FROM tenant_compute WHERE id=NEW.selected_compute_id AND status='enabled' AND retired_at IS NULL) THEN RAISE EXCEPTION 'invalid session compute selection' USING ERRCODE='23514'; END IF;
 ELSIF NEW.selected_compute_id IS NOT NULL THEN RAISE EXCEPTION 'legacy sessions cannot fabricate compute IDs' USING ERRCODE='23514'; END IF;
 RETURN NEW;
END $$;

CREATE FUNCTION public.validate_session_model() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE a JSONB;
BEGIN
 -- Migration 013's session_compute trigger already freezes every identity and
 -- configuration column on UPDATE, including these additive model fields.
 IF TG_OP='UPDATE' THEN RETURN NEW; END IF;
 SELECT manifest INTO a FROM artifact_manifests WHERE revision_id=NEW.revision_id;
 IF a->>'contract_version' IN ('2','3') THEN
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



CREATE TABLE public.artifact_manifests (
    revision_id uuid NOT NULL,
    artifact_digest text NOT NULL,
    manifest jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT artifact_manifests_artifact_digest_check CHECK ((artifact_digest ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT artifact_manifests_manifest_check CHECK ((jsonb_typeof(manifest) = 'object'::text))
);

CREATE TABLE public.chat_evidence_events (
    event_id uuid NOT NULL,
    turn_id uuid NOT NULL,
    execution_id uuid NOT NULL,
    source_digest text NOT NULL,
    disposition text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    source jsonb NOT NULL,
    CONSTRAINT chat_evidence_events_disposition_check CHECK ((disposition = ANY (ARRAY['stored'::text, 'conflict'::text])))
);

CREATE TABLE public.chat_messages (
    id uuid NOT NULL,
    session_id uuid NOT NULL,
    turn_id uuid NOT NULL,
    ordinal bigint NOT NULL,
    role text NOT NULL,
    content text NOT NULL,
    model_message jsonb,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT chat_messages_content_check CHECK ((length(content) <= 16384)),
    CONSTRAINT chat_messages_role_check CHECK ((role = ANY (ARRAY['user'::text, 'assistant'::text, 'tool'::text])))
);

CREATE TABLE public.chat_sessions (
    id uuid NOT NULL,
    created_by text NOT NULL,
    first_key text NOT NULL,
    first_digest text NOT NULL,
    display_name text,
    harness_id uuid NOT NULL,
    deployment_id uuid NOT NULL,
    revision_id uuid NOT NULL,
    sandbox_id uuid,
    state text NOT NULL,
    last_sequence bigint DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    selected_compute_id uuid,
    compute_snapshot jsonb,
    selected_model_id uuid,
    model_snapshot jsonb,
    CONSTRAINT chat_sessions_state_check CHECK ((state = ANY (ARRAY['queued'::text, 'ready'::text, 'interrupted'::text, 'ended'::text]))),
    CONSTRAINT session_compute_pair CHECK (((selected_compute_id IS NULL) = (compute_snapshot IS NULL))),
    CONSTRAINT session_model_pair CHECK (((selected_model_id IS NULL) = (model_snapshot IS NULL)))
);

CREATE TABLE public.chat_tool_calls (
    id uuid NOT NULL,
    turn_id uuid NOT NULL,
    execution_id uuid NOT NULL,
    context_id uuid NOT NULL,
    invocation_index integer NOT NULL,
    tool_name text NOT NULL,
    arguments jsonb NOT NULL,
    state text NOT NULL,
    exit_code integer,
    elapsed_ms bigint,
    stdout_preview text,
    stderr_preview text,
    CONSTRAINT chat_tool_calls_state_check CHECK ((state = ANY (ARRAY['started'::text, 'completed'::text, 'uncertain'::text, 'denied'::text])))
);

CREATE TABLE public.chat_turns (
    id uuid NOT NULL,
    session_id uuid NOT NULL,
    sequence bigint NOT NULL,
    client_key text NOT NULL,
    prompt_digest text NOT NULL,
    state text NOT NULL,
    terminal_reason text,
    execution_id uuid,
    context_id uuid,
    lease_owner text,
    lease_token uuid,
    lease_expires_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT chat_turns_state_check CHECK ((state = ANY (ARRAY['queued'::text, 'running'::text, 'completed'::text, 'failed'::text, 'stopped'::text, 'uncertain'::text])))
);

CREATE TABLE public.compute_operations (
    principal_ref text NOT NULL,
    operation text NOT NULL,
    resource_key text NOT NULL,
    idempotency_key text NOT NULL,
    request_digest text NOT NULL,
    response jsonb,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT compute_operations_idempotency_key_check CHECK (((length(idempotency_key) >= 1) AND (length(idempotency_key) <= 200))),
    CONSTRAINT compute_operations_request_digest_check CHECK ((request_digest ~ '^[0-9a-f]{64}$'::text))
);

CREATE TABLE public.execution_contexts (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    execution_id uuid NOT NULL,
    sequence integer NOT NULL,
    effective_at timestamp with time zone NOT NULL,
    model jsonb NOT NULL,
    available_tools jsonb NOT NULL,
    tools_completeness text NOT NULL,
    authorization_context jsonb NOT NULL,
    environment_refs jsonb DEFAULT '[]'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    compute_context jsonb,
    CONSTRAINT execution_contexts_authorization_context_check CHECK (((jsonb_typeof(authorization_context) = 'object'::text) AND (authorization_context ? 'completeness'::text) AND (jsonb_typeof((authorization_context -> 'completeness'::text)) = 'string'::text) AND ((authorization_context ->> 'completeness'::text) = ANY (ARRAY['unknown'::text, 'partial'::text, 'complete'::text])) AND (authorization_context ? 'bindings'::text) AND (jsonb_typeof((authorization_context -> 'bindings'::text)) = 'array'::text))),
    CONSTRAINT execution_contexts_available_tools_check CHECK ((jsonb_typeof(available_tools) = 'array'::text)),
    CONSTRAINT execution_contexts_environment_refs_check CHECK ((jsonb_typeof(environment_refs) = 'array'::text)),
    CONSTRAINT execution_contexts_model_check CHECK (((jsonb_typeof(model) = 'object'::text) AND (model ? 'model'::text) AND (jsonb_typeof((model -> 'model'::text)) = 'string'::text) AND (length((model ->> 'model'::text)) > 0))),
    CONSTRAINT execution_contexts_sequence_check CHECK ((sequence >= 0)),
    CONSTRAINT execution_contexts_tools_completeness_check CHECK ((tools_completeness = ANY (ARRAY['unknown'::text, 'partial'::text, 'complete'::text])))
);

CREATE TABLE public.executions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    harness_id uuid NOT NULL,
    harness_revision_id uuid NOT NULL,
    deployment_id uuid,
    integration text NOT NULL,
    external_trace_id text NOT NULL,
    initiator_ref text,
    workflow text NOT NULL,
    started_at timestamp with time zone NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.harness_configuration_operations (
    id uuid NOT NULL,
    principal_ref text NOT NULL,
    operation_kind text NOT NULL,
    resource_ref text NOT NULL,
    idempotency_key text NOT NULL,
    changed_surfaces text[] DEFAULT '{}'::text[] NOT NULL,
    request_digest text NOT NULL,
    harness_id uuid NOT NULL,
    response jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT harness_configuration_operations_id_check CHECK ((id <> '00000000-0000-0000-0000-000000000000'::uuid)),
    CONSTRAINT harness_configuration_operations_idempotency_key_check CHECK (((length(idempotency_key) >= 1) AND (length(idempotency_key) <= 200))),
    CONSTRAINT harness_configuration_operations_operation_kind_check CHECK ((operation_kind = ANY (ARRAY['create'::text, 'save'::text, 'activate'::text, 'rollback'::text, 'metadata'::text]))),
    CONSTRAINT harness_configuration_operations_principal_ref_check CHECK ((length(btrim(principal_ref)) > 0)),
    CONSTRAINT harness_configuration_operations_request_digest_check CHECK ((request_digest ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT harness_configuration_operations_response_check CHECK ((jsonb_typeof(response) = 'object'::text))
);

CREATE TABLE public.harness_deployments (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    harness_id uuid NOT NULL,
    revision_id uuid NOT NULL,
    name text NOT NULL,
    assignment_ref text,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE public.harness_revisions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    name text NOT NULL,
    model text NOT NULL,
    tool_set_id bigint,
    status text NOT NULL,
    parent_revision_id uuid,
    surfaces jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    activated_at timestamp with time zone,
    retired_at timestamp with time zone,
    harness_id uuid NOT NULL,
    provenance text DEFAULT 'registered'::text NOT NULL,
    model_configuration jsonb DEFAULT '{}'::jsonb NOT NULL,
    tool_manifest jsonb NOT NULL,
    model_manifest jsonb,
    default_model_id uuid,
    system_prompt text,
    prompt_provenance jsonb,
    compute_manifest jsonb,
    default_compute_id uuid,
    CONSTRAINT compute_pair CHECK (((compute_manifest IS NULL) = (default_compute_id IS NULL))),
    CONSTRAINT harness_revisions_check CHECK ((id <> parent_revision_id)),
    CONSTRAINT harness_revisions_model_configuration_check CHECK ((jsonb_typeof(model_configuration) = 'object'::text)),
    CONSTRAINT harness_revisions_provenance_check CHECK ((provenance = ANY (ARRAY['registered'::text, 'legacy_migration'::text]))),
    CONSTRAINT harness_revisions_tool_manifest_check CHECK ((jsonb_typeof(tool_manifest) = 'array'::text)),
    CONSTRAINT harnesses_status_check CHECK ((status = ANY (ARRAY['draft'::text, 'active'::text, 'rejected'::text, 'retired'::text]))),
    CONSTRAINT revision_model_snapshot CHECK ((((model_manifest IS NULL) AND (default_model_id IS NULL) AND (system_prompt IS NULL) AND (prompt_provenance IS NULL)) OR ((model_manifest IS NOT NULL) AND (default_model_id IS NOT NULL) AND (system_prompt IS NOT NULL) AND (prompt_provenance IS NOT NULL) AND (jsonb_typeof(model_manifest) = 'array'::text) AND (jsonb_array_length(model_manifest) > 0) AND (length(btrim(system_prompt)) > 0) AND (jsonb_typeof(prompt_provenance) = 'object'::text))))
);

CREATE TABLE public.harnesses (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    name text NOT NULL,
    description text,
    workflow text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    definition_revision_id uuid,
    metadata_generation bigint DEFAULT 1 NOT NULL,
    interactive_deployment_id uuid,
    selected_prompt_id uuid,
    CONSTRAINT harnesses_metadata_generation_check CHECK ((metadata_generation > 0))
);

CREATE TABLE public.model_operations (
    principal_ref text NOT NULL,
    operation text NOT NULL,
    resource_key text NOT NULL,
    idempotency_key text NOT NULL,
    request_digest text NOT NULL,
    response jsonb,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT model_operations_operation_check CHECK ((operation = ANY (ARRAY['create'::text, 'update'::text])))
);

CREATE TABLE public.sandboxes (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    provider_id text NOT NULL,
    notes character varying(30),
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    status text,
    create_time_ms bigint,
    end_time_ms bigint,
    metadata jsonb,
    launch_parameters jsonb,
    capabilities jsonb,
    state_transitions jsonb,
    initiator_type text,
    initiator_id text,
    blueprint_id text,
    snapshot_id text,
    failure_reason text,
    shutdown_reason text,
    tunnel jsonb,
    gateway_specs jsonb,
    mcp_specs jsonb,
    template text,
    compute_id uuid,
    compute_binding_ref text,
    CONSTRAINT sandbox_compute_pair CHECK (((compute_id IS NULL) = (compute_binding_ref IS NULL)))
);

CREATE TABLE public.system_prompts (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    harness_id uuid NOT NULL,
    name text NOT NULL,
    content text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT system_prompts_content_check CHECK (((length(btrim(content)) > 0) AND (length(content) <= 100000))),
    CONSTRAINT system_prompts_name_check CHECK (((length(btrim(name)) > 0) AND (length(name) <= 120)))
);

CREATE TABLE public.tenant_compute (
    id uuid NOT NULL,
    display_name text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    backend text NOT NULL,
    binding_ref text NOT NULL,
    configuration jsonb NOT NULL,
    capabilities jsonb NOT NULL,
    status text NOT NULL,
    generation bigint DEFAULT 1 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    retired_at timestamp with time zone,
    CONSTRAINT tenant_compute_backend_check CHECK ((backend = 'apple-container'::text)),
    CONSTRAINT tenant_compute_binding_ref_check CHECK ((binding_ref ~ '^binding:[A-Za-z0-9_.:-]+$'::text)),
    CONSTRAINT tenant_compute_capabilities_check CHECK ((jsonb_typeof(capabilities) = 'object'::text)),
    CONSTRAINT tenant_compute_configuration_check CHECK ((jsonb_typeof(configuration) = 'object'::text)),
    CONSTRAINT tenant_compute_display_name_check CHECK (((length(btrim(display_name)) >= 1) AND (length(btrim(display_name)) <= 512))),
    CONSTRAINT tenant_compute_generation_check CHECK ((generation > 0)),
    CONSTRAINT tenant_compute_id_check CHECK ((id <> '00000000-0000-0000-0000-000000000000'::uuid)),
    CONSTRAINT tenant_compute_status_check CHECK ((status = ANY (ARRAY['enabled'::text, 'disabled'::text])))
);

CREATE TABLE public.tenant_models (
    id uuid NOT NULL,
    source text NOT NULL,
    source_model_id text NOT NULL,
    display_name text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    task_type text NOT NULL,
    model_configuration jsonb NOT NULL,
    connection_ref text,
    capabilities jsonb NOT NULL,
    supported_catalog_id uuid,
    status text NOT NULL,
    generation bigint DEFAULT 1 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT tenant_models_capabilities_check CHECK ((jsonb_typeof(capabilities) = 'object'::text)),
    CONSTRAINT tenant_models_check CHECK (((length(btrim(source)) > 0) AND (length(btrim(source_model_id)) > 0) AND (length(btrim(display_name)) > 0) AND (length(btrim(task_type)) > 0))),
    CONSTRAINT tenant_models_generation_check CHECK ((generation > 0)),
    CONSTRAINT tenant_models_id_check CHECK ((id <> '00000000-0000-0000-0000-000000000000'::uuid)),
    CONSTRAINT tenant_models_model_configuration_check CHECK ((jsonb_typeof(model_configuration) = 'object'::text)),
    CONSTRAINT tenant_models_status_check CHECK ((status = ANY (ARRAY['enabled'::text, 'disabled'::text])))
);

CREATE TABLE public.tool_set_tools (
    tool_set_id bigint NOT NULL,
    tool_id bigint NOT NULL
);

CREATE TABLE public.tool_sets (
    id bigint NOT NULL,
    name text NOT NULL,
    description text
);

ALTER TABLE public.tool_sets ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.tool_sets_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

CREATE TABLE public.tools (
    id bigint NOT NULL,
    name text NOT NULL,
    description text,
    schema jsonb NOT NULL,
    category text,
    implementation_version text,
    configuration jsonb DEFAULT '{}'::jsonb NOT NULL,
    CONSTRAINT tools_configuration_check CHECK ((jsonb_typeof(configuration) = 'object'::text))
);

ALTER TABLE public.tools ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.tools_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE ONLY public.artifact_manifests
    ADD CONSTRAINT artifact_manifests_pkey PRIMARY KEY (revision_id);

ALTER TABLE ONLY public.chat_evidence_events
    ADD CONSTRAINT chat_evidence_events_pkey PRIMARY KEY (event_id);

ALTER TABLE ONLY public.chat_messages
    ADD CONSTRAINT chat_messages_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.chat_messages
    ADD CONSTRAINT chat_messages_session_id_ordinal_key UNIQUE (session_id, ordinal);

ALTER TABLE ONLY public.chat_sessions
    ADD CONSTRAINT chat_sessions_created_by_first_key_key UNIQUE (created_by, first_key);

ALTER TABLE ONLY public.chat_sessions
    ADD CONSTRAINT chat_sessions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.chat_sessions
    ADD CONSTRAINT chat_sessions_sandbox_id_key UNIQUE (sandbox_id);

ALTER TABLE ONLY public.chat_tool_calls
    ADD CONSTRAINT chat_tool_calls_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.chat_tool_calls
    ADD CONSTRAINT chat_tool_calls_turn_id_invocation_index_key UNIQUE (turn_id, invocation_index);

ALTER TABLE ONLY public.chat_turns
    ADD CONSTRAINT chat_turns_execution_id_key UNIQUE (execution_id);

ALTER TABLE ONLY public.chat_turns
    ADD CONSTRAINT chat_turns_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.chat_turns
    ADD CONSTRAINT chat_turns_session_id_client_key_key UNIQUE (session_id, client_key);

ALTER TABLE ONLY public.chat_turns
    ADD CONSTRAINT chat_turns_session_id_sequence_key UNIQUE (session_id, sequence);

ALTER TABLE ONLY public.compute_operations
    ADD CONSTRAINT compute_operations_pkey PRIMARY KEY (principal_ref, operation, resource_key, idempotency_key);

ALTER TABLE ONLY public.execution_contexts
    ADD CONSTRAINT execution_contexts_execution_id_id_key UNIQUE (execution_id, id);

ALTER TABLE ONLY public.execution_contexts
    ADD CONSTRAINT execution_contexts_execution_id_sequence_key UNIQUE (execution_id, sequence);

ALTER TABLE ONLY public.execution_contexts
    ADD CONSTRAINT execution_contexts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.executions
    ADD CONSTRAINT executions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.harness_configuration_operations
    ADD CONSTRAINT harness_configuration_operati_principal_ref_operation_kind__key UNIQUE (principal_ref, operation_kind, resource_ref, idempotency_key);

ALTER TABLE ONLY public.harness_configuration_operations
    ADD CONSTRAINT harness_configuration_operations_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.harness_deployments
    ADD CONSTRAINT harness_deployments_harness_id_id_key UNIQUE (harness_id, id);

ALTER TABLE ONLY public.harness_deployments
    ADD CONSTRAINT harness_deployments_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.harness_revisions
    ADD CONSTRAINT harness_revisions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.harnesses
    ADD CONSTRAINT harnesses_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.model_operations
    ADD CONSTRAINT model_operations_pkey PRIMARY KEY (principal_ref, operation, resource_key, idempotency_key);

ALTER TABLE ONLY public.harness_revisions
    ADD CONSTRAINT revision_identity UNIQUE (harness_id, id);

ALTER TABLE ONLY public.sandboxes
    ADD CONSTRAINT sandboxes_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.system_prompts
    ADD CONSTRAINT system_prompts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.tenant_compute
    ADD CONSTRAINT tenant_compute_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.tenant_models
    ADD CONSTRAINT tenant_models_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.tool_set_tools
    ADD CONSTRAINT tool_set_tools_pkey PRIMARY KEY (tool_set_id, tool_id);

ALTER TABLE ONLY public.tool_sets
    ADD CONSTRAINT tool_sets_pkey PRIMARY KEY (id);

ALTER TABLE ONLY public.tools
    ADD CONSTRAINT tools_pkey PRIMARY KEY (id);

CREATE INDEX chat_evidence_execution ON public.chat_evidence_events USING btree (execution_id);

CREATE INDEX chat_sessions_owner ON public.chat_sessions USING btree (created_by, created_at DESC);

CREATE INDEX chat_sessions_selected_model ON public.chat_sessions USING btree (selected_model_id);

CREATE INDEX chat_turns_queue ON public.chat_turns USING btree (state, created_at);

CREATE INDEX deployments_revision ON public.harness_deployments USING btree (harness_id, revision_id);

CREATE INDEX executions_deployment ON public.executions USING btree (harness_id, deployment_id);

CREATE INDEX executions_revision ON public.executions USING btree (harness_revision_id, started_at);

CREATE INDEX harness_configuration_operations_harness ON public.harness_configuration_operations USING btree (harness_id);

CREATE INDEX harness_revisions_default_model ON public.harness_revisions USING btree (default_model_id);

CREATE INDEX harnesses_selected_prompt ON public.harnesses USING btree (selected_prompt_id);

CREATE INDEX revisions_parent ON public.harness_revisions USING btree (parent_revision_id);

CREATE INDEX revisions_tool_set ON public.harness_revisions USING btree (tool_set_id);

CREATE INDEX system_prompts_harness_created ON public.system_prompts USING btree (harness_id, created_at DESC, id DESC);

CREATE UNIQUE INDEX tenant_compute_backend_current ON public.tenant_compute USING btree (backend) WHERE (retired_at IS NULL);

CREATE INDEX tool_memberships_tool ON public.tool_set_tools USING btree (tool_id);

CREATE TRIGGER actual_compute BEFORE INSERT ON public.execution_contexts FOR EACH ROW EXECUTE FUNCTION public.validate_actual_compute();

CREATE TRIGGER existing_parent_revision BEFORE INSERT ON public.harness_revisions FOR EACH ROW EXECUTE FUNCTION public.require_existing_parent_revision();

CREATE TRIGGER harness_assignment BEFORE INSERT OR UPDATE OF interactive_deployment_id, selected_prompt_id ON public.harnesses FOR EACH ROW EXECUTE FUNCTION public.validate_harness_assignment();

CREATE TRIGGER immutable_artifact BEFORE DELETE OR UPDATE ON public.artifact_manifests FOR EACH ROW EXECUTE FUNCTION public.protect_artifact_record();

CREATE TRIGGER immutable_configuration_operation BEFORE DELETE OR UPDATE ON public.harness_configuration_operations FOR EACH ROW EXECUTE FUNCTION public.protect_execution_record();

CREATE TRIGGER immutable_execution BEFORE DELETE OR UPDATE ON public.executions FOR EACH ROW EXECUTE FUNCTION public.protect_execution_record();

CREATE TRIGGER immutable_execution_context BEFORE DELETE OR UPDATE ON public.execution_contexts FOR EACH ROW EXECUTE FUNCTION public.protect_execution_record();

CREATE TRIGGER immutable_harness_revision BEFORE DELETE OR UPDATE ON public.harness_revisions FOR EACH ROW EXECUTE FUNCTION public.protect_harness_revision();

CREATE TRIGGER model_artifact_agreement BEFORE INSERT ON public.artifact_manifests FOR EACH ROW EXECUTE FUNCTION public.validate_model_artifact();

CREATE TRIGGER registered_model_history BEFORE DELETE ON public.tenant_models FOR EACH ROW EXECUTE FUNCTION public.protect_registered_model();

CREATE TRIGGER revision_compute BEFORE INSERT OR UPDATE ON public.harness_revisions FOR EACH ROW EXECUTE FUNCTION public.validate_revision_compute();

CREATE TRIGGER revision_models BEFORE INSERT ON public.harness_revisions FOR EACH ROW EXECUTE FUNCTION public.validate_revision_models();

CREATE TRIGGER sandbox_compute_owner BEFORE UPDATE ON public.sandboxes FOR EACH ROW EXECUTE FUNCTION public.preserve_sandbox_compute_owner();

CREATE TRIGGER session_compute BEFORE INSERT OR UPDATE ON public.chat_sessions FOR EACH ROW EXECUTE FUNCTION public.validate_session_compute();

CREATE TRIGGER session_model BEFORE INSERT OR UPDATE ON public.chat_sessions FOR EACH ROW EXECUTE FUNCTION public.validate_session_model();

ALTER TABLE ONLY public.artifact_manifests
    ADD CONSTRAINT artifact_manifests_revision_id_fkey FOREIGN KEY (revision_id) REFERENCES public.harness_revisions(id);

ALTER TABLE ONLY public.chat_evidence_events
    ADD CONSTRAINT chat_evidence_events_execution_id_fkey FOREIGN KEY (execution_id) REFERENCES public.executions(id);

ALTER TABLE ONLY public.chat_evidence_events
    ADD CONSTRAINT chat_evidence_events_turn_id_fkey FOREIGN KEY (turn_id) REFERENCES public.chat_turns(id);

ALTER TABLE ONLY public.chat_messages
    ADD CONSTRAINT chat_messages_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.chat_sessions(id);

ALTER TABLE ONLY public.chat_messages
    ADD CONSTRAINT chat_messages_turn_id_fkey FOREIGN KEY (turn_id) REFERENCES public.chat_turns(id);

ALTER TABLE ONLY public.chat_sessions
    ADD CONSTRAINT chat_sessions_harness_id_deployment_id_fkey FOREIGN KEY (harness_id, deployment_id) REFERENCES public.harness_deployments(harness_id, id);

ALTER TABLE ONLY public.chat_sessions
    ADD CONSTRAINT chat_sessions_harness_id_fkey FOREIGN KEY (harness_id) REFERENCES public.harnesses(id);

ALTER TABLE ONLY public.chat_sessions
    ADD CONSTRAINT chat_sessions_harness_id_revision_id_fkey FOREIGN KEY (harness_id, revision_id) REFERENCES public.harness_revisions(harness_id, id);

ALTER TABLE ONLY public.chat_sessions
    ADD CONSTRAINT chat_sessions_sandbox_id_fkey FOREIGN KEY (sandbox_id) REFERENCES public.sandboxes(id);

ALTER TABLE ONLY public.chat_sessions
    ADD CONSTRAINT chat_sessions_selected_compute_id_fkey FOREIGN KEY (selected_compute_id) REFERENCES public.tenant_compute(id);

ALTER TABLE ONLY public.chat_tool_calls
    ADD CONSTRAINT chat_tool_calls_execution_id_context_id_fkey FOREIGN KEY (execution_id, context_id) REFERENCES public.execution_contexts(execution_id, id);

ALTER TABLE ONLY public.chat_tool_calls
    ADD CONSTRAINT chat_tool_calls_execution_id_fkey FOREIGN KEY (execution_id) REFERENCES public.executions(id);

ALTER TABLE ONLY public.chat_tool_calls
    ADD CONSTRAINT chat_tool_calls_turn_id_fkey FOREIGN KEY (turn_id) REFERENCES public.chat_turns(id);

ALTER TABLE ONLY public.chat_turns
    ADD CONSTRAINT chat_turns_execution_id_context_id_fkey FOREIGN KEY (execution_id, context_id) REFERENCES public.execution_contexts(execution_id, id);

ALTER TABLE ONLY public.chat_turns
    ADD CONSTRAINT chat_turns_execution_id_fkey FOREIGN KEY (execution_id) REFERENCES public.executions(id);

ALTER TABLE ONLY public.chat_turns
    ADD CONSTRAINT chat_turns_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.chat_sessions(id);

ALTER TABLE ONLY public.execution_contexts
    ADD CONSTRAINT execution_contexts_execution_id_fkey FOREIGN KEY (execution_id) REFERENCES public.executions(id);

ALTER TABLE ONLY public.executions
    ADD CONSTRAINT executions_harness_id_deployment_id_fkey FOREIGN KEY (harness_id, deployment_id) REFERENCES public.harness_deployments(harness_id, id);

ALTER TABLE ONLY public.executions
    ADD CONSTRAINT executions_harness_id_fkey FOREIGN KEY (harness_id) REFERENCES public.harnesses(id);

ALTER TABLE ONLY public.executions
    ADD CONSTRAINT executions_harness_id_harness_revision_id_fkey FOREIGN KEY (harness_id, harness_revision_id) REFERENCES public.harness_revisions(harness_id, id);

ALTER TABLE ONLY public.harness_configuration_operations
    ADD CONSTRAINT harness_configuration_operations_harness_id_fkey FOREIGN KEY (harness_id) REFERENCES public.harnesses(id);

ALTER TABLE ONLY public.harnesses
    ADD CONSTRAINT harness_definition_owner FOREIGN KEY (id, definition_revision_id) REFERENCES public.harness_revisions(harness_id, id);

ALTER TABLE ONLY public.harness_deployments
    ADD CONSTRAINT harness_deployments_harness_id_fkey FOREIGN KEY (harness_id) REFERENCES public.harnesses(id);

ALTER TABLE ONLY public.harness_deployments
    ADD CONSTRAINT harness_deployments_harness_id_revision_id_fkey FOREIGN KEY (harness_id, revision_id) REFERENCES public.harness_revisions(harness_id, id);

ALTER TABLE ONLY public.harnesses
    ADD CONSTRAINT harness_interactive_assignment_owner FOREIGN KEY (id, interactive_deployment_id) REFERENCES public.harness_deployments(harness_id, id);

ALTER TABLE ONLY public.harness_revisions
    ADD CONSTRAINT harness_revisions_default_compute_id_fkey FOREIGN KEY (default_compute_id) REFERENCES public.tenant_compute(id);

ALTER TABLE ONLY public.harness_revisions
    ADD CONSTRAINT harness_revisions_default_model_id_fkey FOREIGN KEY (default_model_id) REFERENCES public.tenant_models(id);

ALTER TABLE ONLY public.harness_revisions
    ADD CONSTRAINT harness_revisions_harness_id_fkey FOREIGN KEY (harness_id) REFERENCES public.harnesses(id);

ALTER TABLE ONLY public.harness_revisions
    ADD CONSTRAINT harnesses_parent_harness_id_fkey FOREIGN KEY (parent_revision_id) REFERENCES public.harness_revisions(id);

ALTER TABLE ONLY public.harnesses
    ADD CONSTRAINT harnesses_selected_prompt_id_fkey FOREIGN KEY (selected_prompt_id) REFERENCES public.system_prompts(id);

ALTER TABLE ONLY public.harness_revisions
    ADD CONSTRAINT harnesses_tool_set_id_fkey FOREIGN KEY (tool_set_id) REFERENCES public.tool_sets(id);

ALTER TABLE ONLY public.sandboxes
    ADD CONSTRAINT sandboxes_compute_id_fkey FOREIGN KEY (compute_id) REFERENCES public.tenant_compute(id);

ALTER TABLE ONLY public.chat_sessions
    ADD CONSTRAINT session_model_fk FOREIGN KEY (selected_model_id) REFERENCES public.tenant_models(id);

ALTER TABLE ONLY public.system_prompts
    ADD CONSTRAINT system_prompts_harness_id_fkey FOREIGN KEY (harness_id) REFERENCES public.harnesses(id);

ALTER TABLE ONLY public.tool_set_tools
    ADD CONSTRAINT tool_set_tools_tool_id_fkey FOREIGN KEY (tool_id) REFERENCES public.tools(id);

ALTER TABLE ONLY public.tool_set_tools
    ADD CONSTRAINT tool_set_tools_tool_set_id_fkey FOREIGN KEY (tool_set_id) REFERENCES public.tool_sets(id);

