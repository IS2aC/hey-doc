BEGIN;

SELECT pg_advisory_xact_lock(1040937771);
CREATE TABLE IF NOT EXISTS schema_migrations (
    version integer PRIMARY KEY,
    applied_at timestamptz NOT NULL DEFAULT now()
);
SELECT EXISTS (SELECT FROM schema_migrations WHERE version = 1) AS applied \gset
\if :applied
    COMMIT;
    \quit
\endif

CREATE EXTENSION pg_trgm;

CREATE TABLE sources (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    publisher text NOT NULL,
    reference text NOT NULL,
    published_on date,
    UNIQUE (publisher, reference)
);

CREATE TABLE presentations (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    label text NOT NULL,
    ingredient text,
    strength text,
    dosage_form text,
    pack text,
    search_text text GENERATED ALWAYS AS (
        lower(label || ' ' || coalesce(ingredient, '') || ' ' || coalesce(strength, '')
            || ' ' || coalesce(dosage_form, '') || ' ' || coalesce(pack, ''))
    ) STORED,
    CHECK (btrim(label) <> '')
);
CREATE INDEX presentations_search_idx ON presentations USING gin (search_text gin_trgm_ops);

CREATE TABLE presentation_references (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    presentation_id bigint NOT NULL REFERENCES presentations,
    source_id bigint NOT NULL REFERENCES sources,
    source_code text,
    source_row text NOT NULL,
    original_label text NOT NULL,
    page integer CHECK (page > 0),
    UNIQUE (source_id, source_row)
);
CREATE INDEX presentation_references_code_idx ON presentation_references (source_id, source_code);

CREATE TABLE sectors (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    city text NOT NULL,
    district text NOT NULL,
    UNIQUE (city, district)
);

CREATE TABLE pharmacies (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    sector_id bigint NOT NULL REFERENCES sectors,
    name text NOT NULL,
    phone text,
    address text,
    latitude numeric(9,6) CHECK (latitude BETWEEN -90 AND 90),
    longitude numeric(9,6) CHECK (longitude BETWEEN -180 AND 180),
    CHECK ((latitude IS NULL) = (longitude IS NULL)),
    UNIQUE (id, sector_id)
);

CREATE TABLE prices (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    presentation_id bigint NOT NULL REFERENCES presentations,
    source_id bigint NOT NULL REFERENCES sources,
    sector_id bigint REFERENCES sectors,
    pharmacy_id bigint,
    kind text NOT NULL CHECK (kind IN ('cash', 'mugefci_published', 'cmu_contracted')),
    amount_fcfa numeric(12,2) NOT NULL CHECK (amount_fcfa > 0 AND amount_fcfa <> 'NaN'::numeric),
    observed_on date NOT NULL,
    FOREIGN KEY (pharmacy_id, sector_id) REFERENCES pharmacies (id, sector_id),
    CHECK (pharmacy_id IS NULL OR sector_id IS NOT NULL),
    CHECK (kind <> 'cash' OR sector_id IS NOT NULL),
    UNIQUE NULLS NOT DISTINCT (presentation_id, source_id, kind, sector_id, pharmacy_id, observed_on)
);
CREATE INDEX prices_cash_lookup_idx ON prices (presentation_id, sector_id, observed_on DESC) WHERE kind = 'cash';

CREATE TABLE coverage_listings (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    insurer text NOT NULL,
    scheme text NOT NULL,
    source_id bigint NOT NULL REFERENCES sources,
    UNIQUE (insurer, scheme, source_id)
);

CREATE TABLE coverage_entries (
    listing_id bigint NOT NULL REFERENCES coverage_listings,
    presentation_id bigint NOT NULL REFERENCES presentations,
    PRIMARY KEY (listing_id, presentation_id)
);
CREATE INDEX coverage_entries_presentation_idx ON coverage_entries (presentation_id);

CREATE TABLE duty_schedules (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    sector_id bigint NOT NULL REFERENCES sectors,
    source_id bigint NOT NULL REFERENCES sources,
    period tstzrange NOT NULL,
    CHECK (NOT isempty(period) AND NOT lower_inf(period) AND NOT upper_inf(period)
        AND lower_inc(period) AND NOT upper_inc(period)),
    UNIQUE (source_id, sector_id, period),
    UNIQUE (id, sector_id, period)
);
CREATE INDEX duty_schedules_period_idx ON duty_schedules USING gist (period);

CREATE TABLE pharmacy_duties (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    schedule_id bigint NOT NULL,
    sector_id bigint NOT NULL,
    schedule_period tstzrange NOT NULL,
    pharmacy_id bigint NOT NULL,
    period tstzrange NOT NULL,
    FOREIGN KEY (schedule_id, sector_id, schedule_period) REFERENCES duty_schedules (id, sector_id, period),
    FOREIGN KEY (pharmacy_id, sector_id) REFERENCES pharmacies (id, sector_id),
    CHECK (NOT isempty(period) AND lower_inc(period) AND NOT upper_inc(period) AND period <@ schedule_period),
    UNIQUE (schedule_id, pharmacy_id, period)
);
CREATE INDEX pharmacy_duties_period_idx ON pharmacy_duties USING gist (period);

INSERT INTO schema_migrations (version) VALUES (1);
COMMIT;
