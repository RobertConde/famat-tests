-- Admin login: the shared passcode, Admin sessions, and failed-login rate
-- limiting. There are no per-person accounts (one shared passcode).

-- Exactly one row, created here.
CREATE TABLE admin_auth (
    singleton        boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    -- An argon2id PHC string, set with `famat set-passcode`. NULL until then,
    -- and nobody can log in.
    passcode_hash    text CHECK (passcode_hash LIKE '$argon2id$%'),
    passcode_set_at  timestamptz,
    -- The HMAC key that signs CSRF tokens. gen_random_uuid() draws from the
    -- server's strong random source; two of them give 244 random bits.
    csrf_key         bytea NOT NULL CHECK (length(csrf_key) = 32)
);

INSERT INTO admin_auth (csrf_key)
VALUES (decode(replace(gen_random_uuid()::text || gen_random_uuid()::text, '-', ''), 'hex'));

-- A logged-in browser. The cookie holds a random token; only its SHA-256 is
-- stored, so reading this table does not let anyone log in. Setting a new
-- passcode deletes every row.
CREATE TABLE admin_sessions (
    token_sha256  bytea PRIMARY KEY CHECK (length(token_sha256) = 32),
    created_at    timestamptz NOT NULL DEFAULT now(),
    expires_at    timestamptz NOT NULL
);

-- Recent login attempts that failed (or are still being checked), per
-- client: an IP address, or an IPv6 /64. Rows older than the rate-limit
-- window are deleted as new attempts arrive.
CREATE TABLE login_attempts (
    id            bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    client        text NOT NULL,
    attempted_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX login_attempts_client ON login_attempts (client, attempted_at);
