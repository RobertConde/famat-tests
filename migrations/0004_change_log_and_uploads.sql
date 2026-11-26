-- The change log of every Admin write, and uploads held while an Admin
-- decides what to do about a duplicate Test.

-- One row per changed field. There is no "who": everyone shares one
-- passcode. Rows are written by server::change_log in the same transaction
-- as the change itself, so the log never disagrees with the data.
CREATE TABLE change_log (
    id          bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    -- now() is the transaction's start time, so every entry of one Admin
    -- action shares it.
    changed_at  timestamptz NOT NULL DEFAULT now(),
    -- 'test', 'document', 'facet_value', 'document_kind' (server::change_log::Entity).
    entity      text   NOT NULL CHECK (btrim(entity) <> ''),
    entity_id   bigint NOT NULL,
    -- A column-like name: 'year', 'host', 'kind', 'published', 'original'…
    -- A created record has an 'id' entry with no before; a deleted one, an
    -- 'id' entry with no after.
    field       text   NOT NULL CHECK (btrim(field) <> ''),
    -- Human-readable values (Facet value names, not IDs). NULL is unknown or
    -- absent.
    before      text,
    after       text,
    CHECK (before IS DISTINCT FROM after)
);
CREATE INDEX change_log_recent ON change_log (changed_at DESC, id DESC);
CREATE INDEX change_log_entity ON change_log (entity, entity_id);

-- An Add test upload whose files are already in Garage (under Document IDs
-- drawn from the documents sequence) but whose Test matched an existing one,
-- so it waits for the Admin to choose: add the files to that Test, save a
-- new Test anyway, or discard them. `plan` is server::add_test::Plan.
-- Abandoned ones are swept, with their objects, after a week.
CREATE TABLE pending_uploads (
    id          bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    plan        jsonb NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now()
);
