-- One persistent resource scope, with no organization or account tables.
CREATE TABLE local_workspace (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    id uuid NOT NULL UNIQUE DEFAULT gen_random_uuid()
);
INSERT INTO local_workspace DEFAULT VALUES;
CREATE FUNCTION magicmtn_current_organization() RETURNS uuid LANGUAGE sql STABLE AS $$
    SELECT id FROM local_workspace WHERE singleton
$$;
CREATE TABLE platform_workspace (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    organization_id uuid NOT NULL REFERENCES local_workspace(id)
);
INSERT INTO platform_workspace(organization_id) SELECT id FROM local_workspace;

