-- Accounts: a sign-up becomes an account once it has a password (Argon2id, PHC string).
-- People who signed up before this migration have no password until they create one.
ALTER TABLE signups ADD COLUMN password_hash text;

-- Web sessions. The browser keeps a random token in an HttpOnly cookie; only its SHA-256 is stored.
CREATE TABLE sessions (
    token_sha256 text PRIMARY KEY,
    signup_id uuid NOT NULL REFERENCES signups (id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    CHECK (expires_at > created_at)
);

CREATE INDEX sessions_signup_idx ON sessions (signup_id);
CREATE INDEX sessions_expires_idx ON sessions (expires_at);
