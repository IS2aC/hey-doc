-- Consolidated reference data loaded by the pipeline (pipeline/): every health establishment,
-- product attributes, and the source records behind each consolidated row.

ALTER TABLE sources
    ADD COLUMN url text,
    ADD COLUMN kind text;

CREATE TABLE establishments (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    ref text NOT NULL UNIQUE,
    kind text NOT NULL,
    name text NOT NULL,
    address text,
    city text,
    commune text,
    latitude numeric(9,6) CHECK (latitude BETWEEN -90 AND 90),
    longitude numeric(9,6) CHECK (longitude BETWEEN -180 AND 180),
    phones text[] NOT NULL DEFAULT '{}',
    opening_hours jsonb,
    airp_authorization text,
    license_status text,
    license_valid_until date,
    pharmacist text,
    categories text,
    description text,
    quality_flags text[] NOT NULL DEFAULT '{}',
    CHECK ((latitude IS NULL) = (longitude IS NULL)),
    CHECK (btrim(name) <> '')
);
CREATE INDEX establishments_place_idx ON establishments (kind, city, commune);
CREATE INDEX establishments_name_idx ON establishments USING gin (lower(name) gin_trgm_ops);

ALTER TABLE pharmacies ADD COLUMN establishment_id bigint UNIQUE REFERENCES establishments;

-- One row per source record merged into an establishment, with the raw record and the match.
CREATE TABLE establishment_sources (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    establishment_id bigint NOT NULL REFERENCES establishments,
    source_id bigint NOT NULL REFERENCES sources,
    record_key text NOT NULL,
    match_method text NOT NULL,
    match_score numeric(4,3),
    distance_m integer,
    data jsonb NOT NULL,
    UNIQUE (source_id, record_key)
);
CREATE INDEX establishment_sources_establishment_idx ON establishment_sources (establishment_id);

ALTER TABLE presentations
    ADD COLUMN ref text UNIQUE,
    ADD COLUMN therapeutic_group text,
    ADD COLUMN product_type text,
    ADD COLUMN review_note text;

ALTER TABLE presentation_references
    ADD COLUMN match_method text,
    ADD COLUMN match_score numeric(4,3);

ALTER TABLE prices ADD COLUMN review_note text;

ALTER TABLE coverage_entries ADD COLUMN category text;
