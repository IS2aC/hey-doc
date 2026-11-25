-- Chat conversations of signed-in accounts. Photos are never stored: a photo message only
-- keeps what was read from it, in `data`.
CREATE TABLE conversations (
    id uuid PRIMARY KEY,
    signup_id uuid NOT NULL REFERENCES signups (id) ON DELETE CASCADE,
    title text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX conversations_signup_idx ON conversations (signup_id, updated_at DESC);

CREATE TABLE messages (
    id uuid PRIMARY KEY,
    conversation_id uuid NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    role text NOT NULL CHECK (role IN ('user', 'assistant')),
    -- text: a typed message or a model answer. photo: a prescription photo sent by the person.
    -- prescription: the lines read and matched to the catalogue, detailed in `data`.
    kind text NOT NULL DEFAULT 'text' CHECK (kind IN ('text', 'photo', 'prescription')),
    content text NOT NULL,
    data jsonb,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp()
);

CREATE INDEX messages_conversation_idx ON messages (conversation_id, created_at);
