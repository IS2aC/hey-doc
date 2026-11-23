-- Prescriptions archived with the person's consent, to build an annotated corpus.
-- The image and the annotation live in object storage (MinIO / S3); this table indexes them.
-- Only product and dosage text is stored: names, dates and signatures are masked before upload.
CREATE TABLE prescription_reads (
    id uuid PRIMARY KEY,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    channel text NOT NULL CHECK (channel IN ('web', 'whatsapp', 'api')),
    image_key text NOT NULL UNIQUE,
    annotation_key text NOT NULL UNIQUE,
    image_sha256 text NOT NULL,
    width integer NOT NULL,
    height integer NOT NULL,
    provider text NOT NULL,
    model text NOT NULL,
    kept_regions integer NOT NULL,
    masked_regions integer NOT NULL,
    review_status text NOT NULL DEFAULT 'to_review'
        CHECK (review_status IN ('to_review', 'validated', 'rejected')),
    reviewed_at timestamptz,
    search_text text NOT NULL DEFAULT '',
    CHECK (expires_at > created_at)
);

CREATE INDEX prescription_reads_expires_idx ON prescription_reads (expires_at);
CREATE INDEX prescription_reads_review_idx ON prescription_reads (review_status, created_at);
CREATE INDEX prescription_reads_search_idx ON prescription_reads
    USING gin (to_tsvector('simple', search_text));
CREATE INDEX prescription_reads_trgm_idx ON prescription_reads USING gin (lower(search_text) gin_trgm_ops);
