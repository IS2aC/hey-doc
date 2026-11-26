"""Load the consolidated files into PostgreSQL, in one transaction.

The pipeline owns every reference table: they are emptied and reloaded on each run, so a
re-run never duplicates rows. The schema itself comes from the API migrations (SQLx).
"""

from __future__ import annotations

import json
import os
import statistics
from collections import defaultdict
from datetime import date, datetime, timedelta, timezone

import psycopg
from psycopg.types.json import Jsonb

from .common import DATA, OUT, STAGING, norm_coord, norm_phone, read_csv
from .normalize import distance_m

DEFAULT_URL = "postgresql://hey_doc:hey_doc_local@127.0.0.1:5433/hey_doc"
TABLES = ["pharmacy_duties", "duty_schedules", "coverage_entries", "coverage_listings", "prices",
          "presentation_references", "presentations", "establishment_sources", "pharmacies",
          "establishments", "sectors", "sources"]

SOURCES = {
    "airp-officines": ("AIRP", "Registre public des officines agréées", None, "https://sigir.airp.ci/api/public/officines", "registre officiel"),
    "site-annuaire": ("pharmacies-de-garde.ci", "Annuaire des établissements de santé", None, "https://www.pharmacies-de-garde.ci/listing/", "site privé"),
    "site-gardes": ("pharmacies-de-garde.ci", "Pharmacies de garde", None, "https://www.pharmacies-de-garde.ci/liste-des-pharmacies-de-garde-en-cote-divoire/", "site privé"),
    "site-prix": ("pharmacies-de-garde.ci", "Prix des médicaments (reprise de la liste MUGEF-CI)", None, "https://www.pharmacies-de-garde.ci/prix-des-medicaments-en-pharmacie-en-cote-divoire/", "site privé"),
    "site-cmu": ("pharmacies-de-garde.ci", "Médicaments pris en charge par la CMU", None, "https://www.pharmacies-de-garde.ci/liste-des-medicaments-pris-en-charge-par-la-cmu/", "site privé"),
    "mugef-2026-09": ("MUGEF-CI", "Liste des médicaments remboursables, édition septembre 2026", date(2026, 9, 8), None, "document officiel"),
    "mugef-2022-11": ("MUGEF-CI", "Liste des médicaments remboursables, édition novembre 2022", date(2022, 11, 10), None, "document officiel"),
    "cmu-hta-2025": ("AIRP/CNAM/DGCMU", "Décision n° 0016 du 29/09/2025, médicaments du diabète et de l'HTA", date(2025, 9, 29), None, "document officiel (OCR)"),
    # Reference completed with the collection date at load time.
    "gmaps": ("Google", "Google Maps (recherche par officine)", None, "https://www.google.com/maps", "web"),
    "relecture-positions": ("Hey-Doc", "Relecture des positions des pharmacies (review_positions.csv)", None, None, "relecture"),
}
GMAPS_REVIEW = OUT / "review_gmaps.csv"
GMAPS_RESULTS = DATA / "gmaps" / "results.csv"
GMAPS_MAX_KM = 30
COVERAGE_LISTINGS = {
    ("mugef-2026-09", "RCO"): ("MUGEF-CI", "RCO"),
    ("mugef-2026-09", "IVSPL"): ("MUGEF-CI", "IVSPL"),
    ("mugef-2022-11", "CMU"): ("MUGEF-CI", "CMU"),
    # Only two therapeutic classes: kept apart so that `scheme=cmu` does not read it as the full CMU list.
    ("cmu-hta-2025", "CMU"): ("CNAM", "CMU (HTA et diabète)"),
}


def duty_slots(week_start: date) -> list[tuple[datetime, datetime]]:
    """Published rule: 20:00-08:00 on weeknights and the whole weekend, week from Saturday to Friday.

    Abidjan is UTC+0 all year. The weekend block runs from Saturday 08:00 to Monday 08:00.
    """
    utc = timezone.utc
    at = lambda d, h: datetime(d.year, d.month, d.day, h, tzinfo=utc)
    monday = week_start + timedelta(days=2)
    slots = [(at(week_start, 8), at(monday, 8))]
    for i in range(5):
        night = monday + timedelta(days=i)
        slots.append((at(night, 20), at(night + timedelta(days=1), 8)))
    return slots


def _num(v):
    return None if v in (None, "") else v


def apply_gmaps(ests: list[dict], review: list[dict]) -> tuple[list[dict], dict[str, int]]:
    """Fill the empty fields of `ests` (rows of establishments.csv, changed in place) from the
    accepted rows of review_gmaps.csv. An existing value is never replaced.

    Rows are found by `ref`, the only stable key. A Google position more than 30 km from the median
    of the town's known positions skips the whole row. Returns the source links and the counts.
    """
    by_ref = {e["est_id"]: e for e in ests}
    coords: dict[str, list[tuple[float, float]]] = defaultdict(list)
    for e in ests:
        if e["city"] and e["lat"] and e["lng"]:
            coords[e["city"]].append((float(e["lat"]), float(e["lng"])))
    median = {c: (statistics.median(p[0] for p in pts), statistics.median(p[1] for p in pts)) for c, pts in coords.items()}

    links, counts = [], defaultdict(int)
    for r in review:
        if (r.get("accept") or "").strip().lower() != "oui":
            continue
        e = by_ref.get(r.get("ref"))
        if e is None:
            counts["référence inconnue"] += 1
            continue
        lat, lng = norm_coord(r.get("gmaps_lat"), -90, 90), norm_coord(r.get("gmaps_lng"), -180, 180)
        if not (lat and lng):
            lat = lng = None
        elif not (4 <= float(lat) <= 11 and -9 <= float(lng) <= -2):
            counts["position hors de Côte d'Ivoire"] += 1
            continue
        elif e["city"] in median and distance_m(*median[e["city"]], lat, lng) > GMAPS_MAX_KM * 1000:
            counts[f"position à plus de {GMAPS_MAX_KM} km de la médiane de la ville"] += 1
            continue
        changed = False
        if lat and not (e["lat"] and e["lng"]):
            e["lat"], e["lng"] = lat, lng
            counts["gps"] += 1
            changed = True
        address = (r.get("gmaps_address") or "").strip()
        if address and not (e["address"] or "").strip():
            e["address"] = address
            counts["adresse"] += 1
            changed = True
        phone = norm_phone(r.get("gmaps_phone"))
        phones = e["phones"].split()
        if phone and phone not in phones:
            e["phones"] = " ".join([*phones, phone])
            counts["téléphone"] += 1
            changed = True
        flags = [f for f in e["flags"].split(" ; ") if f]
        if changed and "gmaps_enrichi" not in flags:
            e["flags"] = " ; ".join([*flags, "gmaps_enrichi"])
        counts["établissements enrichis" if changed else "rien à compléter"] += 1
        links.append({"est_id": e["est_id"], "record_key": r.get("gmaps_place_id") or f"ref:{e['est_id']}",
                      "match_score": _num(r.get("name_score")), "distance_m": _num(r.get("distance_m")),
                      "data": {k: v for k, v in r.items() if v not in (None, "")}})
    return links, dict(counts)


def apply_positions(ests: list[dict], links: list[dict], communes) -> tuple[list[dict], dict[str, int], set[str]]:
    """Apply the decisions of review_positions.csv to `ests` (changed in place), then flag the
    pharmacies whose position is still doubtful. Returns the review links, the counts and the
    references of the doubtful pharmacies, whose position the API must not use.

    `accept` = oui: the proposed position, or corrected_lat/corrected_lng when filled, replaces the
    current one. `accept` = garder: the current position is right. A decision is ignored when the
    pharmacy behind `ref` or its position changed since the review.
    """
    from .positions import REVIEW, checker, point

    by_ref = {e["est_id"]: e for e in ests}
    out, counts, reviewed = [], defaultdict(int), set()
    for r in read_csv(REVIEW) if REVIEW.exists() else []:
        accept = (r.get("accept") or "").strip().lower()
        if accept not in ("oui", "garder"):
            continue
        e = by_ref.get(r["ref"])
        if e is None or e["name"] != r["name"]:
            counts["référence changée depuis la relecture"] += 1
            continue
        if not (e["lat"] and point(e["lat"], e["lng"]) == point(r["lat"], r["lng"])):
            counts["position changée depuis la relecture"] += 1
            continue
        flags = [f for f in e["flags"].split(" ; ") if f]
        if accept == "garder":
            flags.append("position confirmée après relecture")
            counts["positions confirmées"] += 1
        else:
            manual = bool(r.get("corrected_lat") or r.get("corrected_lng"))
            lat = norm_coord(r.get("corrected_lat") if manual else r.get("proposed_lat"), -90, 90)
            lng = norm_coord(r.get("corrected_lng") if manual else r.get("proposed_lng"), -180, 180)
            if not (lat and lng and 4 <= float(lat) <= 11 and -9 <= float(lng) <= -2):
                counts["position corrigée absente ou hors de Côte d'Ivoire"] += 1
                continue
            source = "saisie manuelle" if manual else r["proposed_source"]
            e["lat"], e["lng"] = lat, lng
            flags.append(f"position corrigée après relecture ({source}), ancienne {r['lat']},{r['lng']}")
            counts["positions corrigées"] += 1
        e["flags"] = " ; ".join(flags)
        reviewed.add(e["est_id"])
        out.append({"est_id": e["est_id"], "data": {k: v for k, v in r.items() if v not in (None, "")}})

    check = checker(ests, links, communes)
    doubtful = set()
    for e in ests:
        if e["kind"] == "pharmacie" and e["est_id"] not in reviewed and (issues := check.issues(e)):
            doubtful.add(e["est_id"])
            e["flags"] = " ; ".join([*(f for f in e["flags"].split(" ; ") if f), "position douteuse : " + ", ".join(issues)])
    counts["positions douteuses écartées"] = len(doubtful)
    return out, dict(counts), doubtful


def run() -> None:
    url = os.environ.get("DATABASE_URL", DEFAULT_URL)
    ests = read_csv(OUT / "establishments.csv")
    est_links = read_csv(OUT / "establishment_links.csv")
    duties = read_csv(OUT / "duties.csv")
    pres = read_csv(OUT / "presentations.csv")
    refs = read_csv(OUT / "presentation_refs.csv")
    prices = read_csv(OUT / "product_prices.csv")
    coverage = read_csv(OUT / "product_coverage.csv")
    raw = {
        "airp-officines": {r["airp_id"]: r for r in read_csv(STAGING / "airp_officines.csv")},
        "site-annuaire": {r["listing_id"]: r for r in read_csv(STAGING / "site_directory.csv")},
        "site-gardes": {f"{r['zone']}#{r['source_row']}": r for r in read_csv(STAGING / "site_duties.csv")},
    }
    week_start = date.fromisoformat(duties[0]["week_start"]) if duties else None
    gmaps_links, gmaps_counts = apply_gmaps(ests, read_csv(GMAPS_REVIEW)) if GMAPS_REVIEW.exists() else ([], {})
    from .positions import load_communes
    position_links, position_counts, doubtful = apply_positions(ests, est_links, load_communes())
    collected = GMAPS_RESULTS if GMAPS_RESULTS.exists() else GMAPS_REVIEW
    gmaps_date = date.fromtimestamp(collected.stat().st_mtime) if collected.exists() else None

    with psycopg.connect(url) as conn, conn.cursor() as cur:
        cur.execute(f"TRUNCATE {', '.join(TABLES)} RESTART IDENTITY CASCADE")

        src = {}
        for key, (publisher, reference, published, src_url, kind) in SOURCES.items():
            if key == "site-gardes" and week_start:
                reference = f"{reference}, semaine du {week_start.isoformat()}"
            if key == "gmaps" and gmaps_date:
                reference = f"{reference}, collecte du {gmaps_date.isoformat()}"
            cur.execute("INSERT INTO sources (publisher, reference, published_on, url, kind) VALUES (%s, %s, %s, %s, %s) RETURNING id",
                        (publisher, reference, published, src_url, kind))
            src[key] = cur.fetchone()[0]

        est_id = {}
        for e in ests:
            cur.execute(
                """INSERT INTO establishments (ref, kind, name, address, city, commune, latitude, longitude, phones,
                       opening_hours, airp_authorization, license_status, license_valid_until, pharmacist, categories,
                       description, quality_flags)
                   VALUES (%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s) RETURNING id""",
                (e["est_id"], e["kind"], e["name"], _num(e["address"]), _num(e["city"]), _num(e["commune"]),
                 _num(e["lat"]), _num(e["lng"]), e["phones"].split(), Jsonb(json.loads(e["opening_hours"])) if e["opening_hours"] else None,
                 _num(e["airp_authorization"]), _num(e["license_status"]), _num(e["license_valid_until"]),
                 _num(e["pharmacist"]), _num(e["categories"]), _num(e["description"]),
                 [f for f in e["flags"].split(" ; ") if f]))
            est_id[e["est_id"]] = cur.fetchone()[0]

        for l in est_links:
            data = raw.get(l["source"], {}).get(l["source_record"], {})
            cur.execute(
                """INSERT INTO establishment_sources (establishment_id, source_id, record_key, match_method, match_score, distance_m, data)
                   VALUES (%s,%s,%s,%s,%s,%s,%s)""",
                (est_id[l["est_id"]], src[l["source"]], l["source_record"], l["match_method"], _num(l["match_score"]),
                 _num(l["distance_m"]), Jsonb({k: v for k, v in data.items() if v not in (None, "")})))
        for l in gmaps_links:
            cur.execute(
                """INSERT INTO establishment_sources (establishment_id, source_id, record_key, match_method, match_score, distance_m, data)
                   VALUES (%s,%s,%s,'gmaps_search',%s,%s,%s) ON CONFLICT DO NOTHING""",
                (est_id[l["est_id"]], src["gmaps"], l["record_key"], l["match_score"],
                 round(float(l["distance_m"])) if l["distance_m"] else None, Jsonb(l["data"])))
        for l in position_links:
            cur.execute(
                """INSERT INTO establishment_sources (establishment_id, source_id, record_key, match_method, data)
                   VALUES (%s,%s,%s,'position_review',%s)""",
                (est_id[l["est_id"]], src["relecture-positions"], l["est_id"], Jsonb(l["data"])))

        # Operational pharmacies (used by the API) are grouped by sector: commune in Abidjan, town elsewhere.
        sector_id: dict[tuple[str, str], int] = {}

        def sector(city: str | None, commune: str | None) -> int:
            city = city or "Ville non précisée"
            district = commune or (f"{city} (commune non précisée)" if city == "Abidjan" else city)
            if (city, district) not in sector_id:
                cur.execute("INSERT INTO sectors (city, district) VALUES (%s, %s) RETURNING id", (city, district))
                sector_id[(city, district)] = cur.fetchone()[0]
            return sector_id[(city, district)]

        # A doubtful position stays on the establishment, with its flag, but not on the pharmacy:
        # the API sorts pharmacies by distance and gives directions from this position.
        pharmacy: dict[str, tuple[int, int]] = {}
        for e in ests:
            if e["kind"] != "pharmacie":
                continue
            sid = sector(e["city"], e["commune"])
            lat, lng = (None, None) if e["est_id"] in doubtful else (_num(e["lat"]), _num(e["lng"]))
            cur.execute(
                """INSERT INTO pharmacies (sector_id, name, phone, address, latitude, longitude, establishment_id)
                   VALUES (%s,%s,%s,%s,%s,%s,%s) RETURNING id""",
                (sid, e["name"], (e["phones"].split() or [None])[0], _num(e["address"]), lat, lng, est_id[e["est_id"]]))
            pharmacy[e["est_id"]] = (cur.fetchone()[0], sid)

        if duties and week_start:
            # Every sector of a town covered by the published list gets a schedule, so that a sector
            # with no pharmacy on duty reads as "none in schedule" rather than "no schedule".
            period = duty_slots(week_start)
            sched_range = (period[0][0], period[-1][1])
            covered_cities = {d["city"] for d in duties}
            schedule: dict[int, int] = {}
            for (city, _), sid in sector_id.items():
                if city in covered_cities:
                    cur.execute("INSERT INTO duty_schedules (sector_id, source_id, period) VALUES (%s, %s, tstzrange(%s, %s)) RETURNING id",
                                (sid, src["site-gardes"], *sched_range))
                    schedule[sid] = cur.fetchone()[0]
            n_slots = 0
            for d in duties:
                ph = pharmacy.get(d["est_id"])
                if ph is None or ph[1] not in schedule:
                    continue
                for start, end in period:
                    cur.execute(
                        """INSERT INTO pharmacy_duties (schedule_id, sector_id, schedule_period, pharmacy_id, period)
                           VALUES (%s, %s, tstzrange(%s, %s), %s, tstzrange(%s, %s)) ON CONFLICT DO NOTHING""",
                        (schedule[ph[1]], ph[1], *sched_range, ph[0], start, end))
                    n_slots += cur.rowcount

        pres_id = {}
        for p in pres:
            cur.execute(
                """INSERT INTO presentations (ref, label, ingredient, strength, dosage_form, pack, therapeutic_group, product_type, review_note)
                   VALUES (%s,%s,%s,%s,%s,%s,%s,%s,%s) RETURNING id""",
                (p["pres_id"], p["label"], _num(p["ingredient"]), _num(p["strength"]), _num(p["dosage_form"]), _num(p["pack"]),
                 _num(p["therapeutic_group"]), _num(p["product_type"]), _num(p["review"])))
            pres_id[p["pres_id"]] = cur.fetchone()[0]
        for r in refs:
            cur.execute(
                """INSERT INTO presentation_references (presentation_id, source_id, source_code, source_row, original_label, page, match_method, match_score)
                   VALUES (%s,%s,%s,%s,%s,%s,%s,%s) ON CONFLICT DO NOTHING""",
                (pres_id[r["pres_id"]], src[r["source"]], _num(r["source_code"]), r["source_record"], r["original_label"] or "?",
                 _num(r["page"]), r["match_method"], _num(r["match_score"])))
        skipped = defaultdict(int)
        for p in prices:
            if not p["amount_fcfa"] or not p["observed_on"]:
                skipped["prix sans montant ou date"] += 1
                continue
            cur.execute(
                """INSERT INTO prices (presentation_id, source_id, kind, amount_fcfa, observed_on, review_note)
                   VALUES (%s,%s,%s,%s,%s,%s) ON CONFLICT DO NOTHING""",
                (pres_id[p["pres_id"]], src[p["source"]], p["kind"], p["amount_fcfa"], p["observed_on"], _num(p["review"])))
            if cur.rowcount == 0:
                skipped["prix en double pour la même présentation et la même source"] += 1
        listing = {}
        for (source, scheme), (insurer, label) in COVERAGE_LISTINGS.items():
            cur.execute("INSERT INTO coverage_listings (insurer, scheme, source_id) VALUES (%s,%s,%s) RETURNING id",
                        (insurer, label, src[source]))
            listing[(source, scheme)] = cur.fetchone()[0]
        for c in coverage:
            lid = listing.get((c["source"], c["scheme"]))
            if lid is None:
                skipped[f"régime inconnu {c['source']}/{c['scheme']}"] += 1
                continue
            cur.execute("INSERT INTO coverage_entries (listing_id, presentation_id, category) VALUES (%s,%s,%s) ON CONFLICT DO NOTHING",
                        (lid, pres_id[c["pres_id"]], _num(c["category"])))

        conn.commit()
        cur.execute("""SELECT (SELECT count(*) FROM establishments), (SELECT count(*) FROM pharmacies),
                              (SELECT count(*) FROM sectors), (SELECT count(*) FROM pharmacy_duties),
                              (SELECT count(*) FROM presentations), (SELECT count(*) FROM prices),
                              (SELECT count(*) FROM coverage_entries), (SELECT count(*) FROM establishment_sources)""")
        e, ph, sec, du, pr, px, cov, links = cur.fetchone()
        print(f"chargé : {e} établissements dont {ph} pharmacies, {sec} secteurs, {du} créneaux de garde, "
              f"{pr} présentations, {px} prix, {cov} couvertures, {links} enregistrements sources")
        for k, v in skipped.items():
            print(f"  ignoré : {v} {k}")
        print("  positions : " + ", ".join(f"{k} {v}" for k, v in sorted(position_counts.items())))
        if gmaps_counts:
            print("  Google Maps : " + ", ".join(f"{k} {v}" for k, v in sorted(gmaps_counts.items())))
