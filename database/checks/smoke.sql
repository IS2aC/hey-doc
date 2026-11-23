BEGIN;
SET LOCAL TIME ZONE 'Africa/Abidjan';

DO $$
DECLARE
    demo_source bigint;
    product bigint;
    sector bigint;
    other_sector bigint;
    pharmacy bigint;
    schedule bigint;
    listing bigint;
    starts_at constant timestamptz := '2026-11-22 19:00:00+00';
    ends_at constant timestamptz := '2026-11-23 08:00:00+00';
BEGIN
    INSERT INTO sources (publisher, reference) VALUES ('Test fictif', 'smoke')
    RETURNING id INTO demo_source;
    INSERT INTO presentations (label, ingredient, strength, dosage_form, pack)
    VALUES ('Produit Démo', 'Substance fictive', '500 mg', 'Comprimé', 'Boîte de 10')
    RETURNING id INTO product;
    INSERT INTO presentation_references (presentation_id, source_id, source_code, source_row, original_label)
    VALUES (product, demo_source, '00123', '1', 'Produit Démo 500 mg B/10');

    IF NOT EXISTS (SELECT FROM presentations WHERE id = product AND search_text LIKE '%500 mg%')
        OR NOT EXISTS (SELECT FROM presentation_references WHERE presentation_id = product AND source_code = '00123') THEN
        RAISE EXCEPTION 'Recherche catalogue ou préservation du code source en échec';
    END IF;

    INSERT INTO sectors (city, district) VALUES ('Ville fictive', 'Secteur A') RETURNING id INTO sector;
    INSERT INTO sectors (city, district) VALUES ('Ville fictive', 'Secteur B') RETURNING id INTO other_sector;
    INSERT INTO pharmacies (sector_id, name) VALUES (sector, 'Officine fictive') RETURNING id INTO pharmacy;

    INSERT INTO prices (presentation_id, source_id, sector_id, pharmacy_id, kind, amount_fcfa, observed_on)
    VALUES (product, demo_source, sector, pharmacy, 'cash', 2400, '2026-11-22');
    INSERT INTO prices (presentation_id, source_id, kind, amount_fcfa, observed_on)
    VALUES (product, demo_source, 'cmu_contracted', 1550.25, '2026-11-22');

    IF (SELECT sum(amount_fcfa * 2) FROM prices WHERE presentation_id = product
            AND sector_id = sector AND kind = 'cash') IS DISTINCT FROM 4800::numeric THEN
        RAISE EXCEPTION 'Estimation comptant incorrecte';
    END IF;

    BEGIN
        INSERT INTO prices (presentation_id, source_id, sector_id, pharmacy_id, kind, amount_fcfa, observed_on)
        VALUES (product, demo_source, other_sector, pharmacy, 'cash', 2400, '2026-11-22');
        RAISE EXCEPTION 'Un prix comptant a été accepté dans un secteur incompatible';
    EXCEPTION WHEN foreign_key_violation THEN
        NULL;
    END;

    INSERT INTO coverage_listings (insurer, scheme, source_id)
    VALUES ('CNAM fictive', 'CMU', demo_source) RETURNING id INTO listing;
    INSERT INTO coverage_entries (listing_id, presentation_id) VALUES (listing, product);

    INSERT INTO duty_schedules (sector_id, source_id, period)
    VALUES (sector, demo_source, tstzrange(starts_at, ends_at))
    RETURNING id INTO schedule;
    INSERT INTO pharmacy_duties (schedule_id, sector_id, schedule_period, pharmacy_id, period)
    VALUES (schedule, sector, tstzrange(starts_at, ends_at), pharmacy, tstzrange(starts_at, ends_at));

    IF NOT EXISTS (SELECT FROM pharmacy_duties WHERE schedule_id = schedule
            AND period @> '2026-11-23 01:00:00+00'::timestamptz)
        OR EXISTS (SELECT FROM pharmacy_duties WHERE schedule_id = schedule AND period @> ends_at) THEN
        RAISE EXCEPTION 'Bornes de la garde incorrectes';
    END IF;

    BEGIN
        INSERT INTO pharmacy_duties (schedule_id, sector_id, schedule_period, pharmacy_id, period)
        VALUES (schedule, sector, tstzrange(starts_at, ends_at), pharmacy,
            tstzrange(starts_at, ends_at + interval '1 hour'));
        RAISE EXCEPTION 'Une garde en dehors du planning a été acceptée';
    EXCEPTION WHEN check_violation THEN
        NULL;
    END;

    RAISE NOTICE 'Catalogue, prix comptant, couverture et garde après minuit vérifiés';
END
$$;

ROLLBACK;
