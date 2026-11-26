-- Files uploaded one per request ahead of the form that saves them (issue
-- #48): with JavaScript, Add test, + Add files and Replace original send
-- each file on its own, so no request is over Cloudflare's 100 MB, and then
-- post the form with these rows' IDs instead of the bytes.
--
-- The object is at `staged/{id}` in Garage. The row is written before the
-- object, and deleted only after it, so every staged object has a row; one
-- never saved is swept with its object after a day (server::staging).
CREATE TABLE staged_files (
    -- Random, so an ID can't be guessed; also the object key's last part.
    id            text PRIMARY KEY CHECK (id ~ '^[0-9a-f]{32}$'),
    -- The name it was uploaded with, for kind guessing, pairing and the
    -- change log.
    name          text NOT NULL,
    extension     text NOT NULL,
    content_type  text NOT NULL,
    -- NULL while the file is still arriving.
    byte_size     bigint,
    sha256        bytea,
    -- Set while a form saving it is being handled, so two can't both use it.
    claimed_at    timestamptz,
    created_at    timestamptz NOT NULL DEFAULT now(),
    CHECK ((byte_size IS NULL) = (sha256 IS NULL))
);
CREATE INDEX staged_files_created ON staged_files (created_at);
