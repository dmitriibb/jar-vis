CREATE EXTENSION IF NOT EXISTS vector;

CREATE TABLE documents (
    id uuid PRIMARY KEY,
    source_key text NOT NULL UNIQUE,
    title text NOT NULL,
    game text,
    edition text,
    active_job_id uuid,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE ingestion_jobs (
    sequence bigint GENERATED ALWAYS AS IDENTITY UNIQUE,
    id uuid PRIMARY KEY,
    document_id uuid NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    fingerprint text NOT NULL,
    checksum text NOT NULL,
    filename text NOT NULL,
    title text NOT NULL,
    game text,
    edition text,
    state text NOT NULL DEFAULT 'queued' CHECK (state IN ('queued', 'running', 'succeeded', 'failed')),
    error text,
    section_count integer NOT NULL DEFAULT 0,
    created_at timestamptz NOT NULL DEFAULT now(),
    finished_at timestamptz
);
ALTER TABLE documents ADD CONSTRAINT active_job_fk
    FOREIGN KEY (active_job_id) REFERENCES ingestion_jobs(id) ON DELETE SET NULL;
CREATE INDEX jobs_pending ON ingestion_jobs(sequence) WHERE state = 'queued';
CREATE INDEX jobs_document ON ingestion_jobs(document_id, sequence DESC);

CREATE TABLE chunks (
    id uuid PRIMARY KEY,
    document_id uuid NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    job_id uuid NOT NULL REFERENCES ingestion_jobs(id) ON DELETE CASCADE,
    ordinal integer NOT NULL,
    heading text NOT NULL,
    page integer,
    text text NOT NULL,
    search_vector tsvector GENERATED ALWAYS AS
        (setweight(to_tsvector('english', heading), 'A') || to_tsvector('english', text)) STORED,
    UNIQUE(job_id, ordinal)
);
CREATE INDEX chunks_search ON chunks USING gin(search_vector);
CREATE INDEX chunks_document ON chunks(document_id, ordinal);

-- Durable outbox for filesystem deletion: retry on restart after a partial failure.
CREATE TABLE file_cleanup (
    document_id uuid PRIMARY KEY
);
