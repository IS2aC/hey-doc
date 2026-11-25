-- People who signed up on the landing page for access to the Hey-Doc chat.
-- One row per WhatsApp number: signing up again updates the row.
CREATE TABLE signups (
    id uuid PRIMARY KEY,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    first_name text NOT NULL CHECK (length(first_name) BETWEEN 1 AND 80),
    last_name text NOT NULL CHECK (length(last_name) BETWEEN 1 AND 80),
    -- ISO 3166-1 alpha-2, for example CI.
    country text NOT NULL CHECK (country ~ '^[A-Z]{2}$'),
    city text NOT NULL CHECK (length(city) BETWEEN 1 AND 80),
    -- E.164, for example +2250701020304.
    whatsapp text NOT NULL UNIQUE CHECK (whatsapp ~ '^\+[1-9][0-9]{7,14}$'),
    email text CHECK (email IS NULL OR length(email) <= 254),
    -- When the person accepted that Hey-Doc keeps these details and contacts them on WhatsApp.
    consent_at timestamptz NOT NULL,
    source text NOT NULL DEFAULT 'landing'
);
