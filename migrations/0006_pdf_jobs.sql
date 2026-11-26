-- The PDF conversion queue (server::pdf). A Document whose pdf_status is
-- 'preparing' has one job here until its PDF is made, it fails for good, or
-- an Admin uploads a PDF; the row is deleted in the same transaction that
-- records the outcome.
--
-- Workers claim a job with FOR UPDATE SKIP LOCKED and lease it: `lease` is a
-- random token and `leased_until` when the lease runs out. A worker that
-- dies leaves its lease to run out, and the job is claimed again. A job is
-- tried at most twice (one retry); `attempts` counts claims.
CREATE TABLE pdf_jobs (
    id            bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    document_id   bigint NOT NULL UNIQUE REFERENCES documents (id) ON DELETE CASCADE,
    attempts      smallint NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    run_after     timestamptz NOT NULL DEFAULT now(),
    lease         uuid,
    leased_until  timestamptz,
    -- Why the previous attempt failed, for the final failure's comment.
    last_error    text,
    created_at    timestamptz NOT NULL DEFAULT now(),
    CHECK ((lease IS NULL) = (leased_until IS NULL))
);
CREATE INDEX pdf_jobs_due ON pdf_jobs (run_after, id);

-- At startup every preparing Document without a job gets one.
CREATE INDEX documents_preparing ON documents (id) WHERE pdf_status = 'preparing';
