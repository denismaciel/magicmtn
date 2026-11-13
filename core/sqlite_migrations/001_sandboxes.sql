-- A single installation identity is a resource namespace, not an organization.
CREATE TABLE installation (singleton INTEGER PRIMARY KEY CHECK(singleton=1), id TEXT NOT NULL);
CREATE TABLE provider_connections (
    id TEXT PRIMARY KEY,
    reference TEXT NOT NULL UNIQUE,
    provider TEXT NOT NULL,
    binding TEXT NOT NULL CHECK(json_valid(binding)),
    status TEXT NOT NULL DEFAULT 'enabled' CHECK(status IN ('enabled','disabled'))
);
CREATE TABLE sandboxes (
    id TEXT PRIMARY KEY,
    record TEXT NOT NULL CHECK(json_valid(record) AND json_extract(record,'$.id')=id),
    status TEXT GENERATED ALWAYS AS (json_extract(record,'$.status')) VIRTUAL,
    provider_id TEXT GENERATED ALWAYS AS (json_extract(record,'$.provider_id')) VIRTUAL,
    expires_at TEXT GENERATED ALWAYS AS (json_extract(record,'$.expires_at')) VIRTUAL
);
CREATE INDEX sandboxes_expiry ON sandboxes(expires_at) WHERE status<>'ended';
CREATE TABLE provider_resource_receipts (
    scope TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind='sandbox'),
    binding TEXT NOT NULL CHECK(json_valid(binding)),
    state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','completed','uncertain')),
    result TEXT CHECK(result IS NULL OR json_valid(result)),
    PRIMARY KEY(scope,kind)
);
CREATE TABLE sandbox_lifecycle_jobs (
    sandbox_id TEXT PRIMARY KEY REFERENCES sandboxes(id),
    operation TEXT NOT NULL CHECK(operation IN ('inspect','terminate')),
    provider_pin TEXT,
    provider_scope TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts>=0),
    next_attempt_at INTEGER NOT NULL DEFAULT (unixepoch()),
    lease_token TEXT,
    lease_expires_at INTEGER,
    last_error TEXT,
    completed_at INTEGER,
    CHECK((lease_token IS NULL)=(lease_expires_at IS NULL))
);
CREATE INDEX sandbox_lifecycle_jobs_due ON sandbox_lifecycle_jobs(next_attempt_at) WHERE completed_at IS NULL;
