"""Match the Google Maps search results (google-maps-scraper-kit, one keyword per pharmacy) to
the pharmacies that lack a position or an address, and write a review file.

Inputs in data/gmaps/: targets.csv (the pharmacies), keyword_map.csv (keyword -> establishment)
and results.csv (the kit's output, its input_id column is the keyword). Nothing is loaded into the
database here: a person checks review_gmaps.csv first. The scraped values are third-party text and
are only compared in code; this step prints counts, never the values.
"""

from __future__ import annotations

from .common import DATA, OUT, norm_coord, norm_phone, read_csv, split_phones, write_csv
from .normalize import city_key, distance_m, name_similarity

GMAPS = DATA / "gmaps"
FIELDS = [
    "establishment_id", "ref", "name", "commune", "city", "missing", "confidence", "reasons",
    "gmaps_title", "gmaps_category", "gmaps_address", "gmaps_phone", "gmaps_lat", "gmaps_lng",
    "gmaps_place_id", "gmaps_link", "name_score", "distance_m", "candidates", "accept",
]


def _score(t: dict, r: dict) -> tuple[str, float, float | None, list[str]]:
    sim = name_similarity(t["name"], r.get("title") or "")
    reasons = [f"nom {sim:.2f}"]
    category = (r.get("category") or "").lower()
    # The engine sometimes cannot read the category: unknown is not "not a pharmacy".
    is_pharmacy = not category or "pharm" in category
    if not category:
        reasons.append("catégorie inconnue")
    elif not is_pharmacy:
        reasons.append("pas une pharmacie")
    ours = {norm_phone(p) for p in split_phones(t.get("phones"))} - {None}
    phone_ok = bool(ours) and norm_phone(r.get("phone")) in ours
    if phone_ok:
        reasons.append("même téléphone")
    place = city_key(" ".join(filter(None, [r.get("address"), r.get("complete_address")])))
    in_place = any(k and k in place for k in (city_key(t["commune"]), city_key(t["city"])))
    if in_place:
        reasons.append("commune ou ville dans l'adresse")
    lat = norm_coord(r.get("latitude"), -90, 90)
    lng = norm_coord(r.get("longitude"), -180, 180)
    dist = distance_m(t["latitude"], t["longitude"], lat, lng) if t["latitude"] and lat else None
    if dist is not None:
        reasons.append(f"{dist:.0f} m de notre position")
    if not is_pharmacy or (lat and not (4 <= float(lat) <= 11 and -9 <= float(lng) <= -2)):
        level = "rejeté"
    elif phone_ok or (sim >= 0.85 and (in_place or (dist is not None and dist < 1000))):
        level = "haute"
    elif sim >= 0.6 and (in_place or dist is None or dist < 5000):
        level = "moyenne"
    else:
        level = "faible"
    return level, sim, dist, reasons


RANK = {"haute": 3, "moyenne": 2, "faible": 1, "rejeté": 0}


def run() -> None:
    targets = {t["establishment_id"]: t for t in read_csv(GMAPS / "targets.csv")}
    by_kw = {m["keyword"]: m["establishment_id"] for m in read_csv(GMAPS / "keyword_map.csv")}
    found: dict[str, list[dict]] = {}
    for r in read_csv(GMAPS / "results.csv"):
        if r.get("input_id") in by_kw:
            found.setdefault(by_kw[r["input_id"]], []).append(r)

    rows, counts = [], {}
    for eid, t in targets.items():
        missing = "+".join(m for m, gap in (("gps", not t["latitude"]), ("adresse", not (t["address"] or "").strip())) if gap)
        base = {"establishment_id": eid, "ref": t["ref"], "name": t["name"], "commune": t["commune"],
                "city": t["city"], "missing": missing, "candidates": len(found.get(eid, []))}
        scored = sorted(((_score(t, r), r) for r in found.get(eid, [])),
                        key=lambda x: (RANK[x[0][0]], x[0][1]), reverse=True)
        if not scored:
            rows.append({**base, "confidence": "introuvable"})
            counts["introuvable"] = counts.get("introuvable", 0) + 1
            continue
        (level, sim, dist, reasons), r = scored[0]
        # Two different places that both pass as "haute" cannot be told apart automatically.
        if level == "haute" and sum(1 for s, _ in scored if s[0] == "haute") > 1:
            level, reasons = "moyenne", [*reasons, "plusieurs candidats"]
        counts[level] = counts.get(level, 0) + 1
        rows.append({
            **base, "confidence": level, "reasons": "; ".join(reasons),
            "gmaps_title": r.get("title"), "gmaps_category": r.get("category"),
            "gmaps_address": r.get("address") or r.get("complete_address"), "gmaps_phone": r.get("phone"),
            "gmaps_lat": norm_coord(r.get("latitude"), -90, 90), "gmaps_lng": norm_coord(r.get("longitude"), -180, 180),
            "gmaps_place_id": r.get("place_id"), "gmaps_link": r.get("link"),
            "name_score": f"{sim:.2f}", "distance_m": f"{dist:.0f}" if dist is not None else "",
            "accept": "oui" if level == "haute" else "",
        })
    rows.sort(key=lambda x: (-RANK.get(x["confidence"], -1), x["city"], x["name"]))
    path = write_csv(OUT / "review_gmaps.csv", rows, FIELDS)
    print(f"Google Maps : {len(rows)} pharmacies, " + ", ".join(f"{k} {v}" for k, v in sorted(counts.items())))
    print(f"Relecture : {path}")
