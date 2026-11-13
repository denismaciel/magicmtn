-- Persist installed implementation selection and fence tool dispatch across lost responses.
ALTER TABLE chat_sessions ADD COLUMN provider_bindings jsonb;
ALTER TABLE chat_tool_calls ADD COLUMN dispatched_at timestamptz;
CREATE FUNCTION guard_provider_bindings() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF OLD.provider_bindings IS NOT NULL AND NEW.provider_bindings IS DISTINCT FROM OLD.provider_bindings THEN
    RAISE EXCEPTION 'session provider bindings are immutable' USING ERRCODE='23514';
  END IF;
  RETURN NEW;
END $$;
CREATE TRIGGER chat_session_provider_bindings_immutable BEFORE UPDATE OF provider_bindings ON chat_sessions
FOR EACH ROW EXECUTE FUNCTION guard_provider_bindings();

ALTER TABLE chat_sessions ADD COLUMN provider_create_started boolean NOT NULL DEFAULT false;
ALTER TABLE tenant_compute DROP CONSTRAINT tenant_compute_backend_check;
ALTER TABLE tenant_compute ADD CONSTRAINT tenant_compute_backend_check CHECK (backend ~ '^[a-z][a-z0-9-]{0,63}$');
CREATE OR REPLACE FUNCTION public.ck_artifact_v2(a jsonb) RETURNS void
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
     IF NOT coalesce(selected->'model_configuration'->>'provider' ~ '^[a-z][a-z0-9-]{0,63}$',false) OR selected->'connection_ref'='null'
     OR NOT (selected->'capabilities'->'tasks' ? 'chat') OR selected->'capabilities'->'tool_calls' IS DISTINCT FROM 'true' THEN RAISE EXCEPTION 'incompatible default model' USING ERRCODE='23514'; END IF;
   END IF;
 ELSE RAISE EXCEPTION 'unknown runtime' USING ERRCODE='23514';
 END CASE;
END $_$;

CREATE OR REPLACE FUNCTION public.ck_compute_manifest_legacy(m jsonb, d uuid) RETURNS void
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
   OR NOT coalesce(s->>'backend' ~ '^[a-z][a-z0-9-]{0,63}$',false) OR s->>'binding_ref' !~ '^binding:[A-Za-z0-9_.:-]+$'
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

