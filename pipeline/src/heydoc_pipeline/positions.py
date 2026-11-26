"""Check the GPS positions of pharmacies and propose corrections for review.

A position is doubtful when it falls outside the commune of its address (Abidjan, OpenStreetMap
boundaries), far from its town elsewhere, or when it is shared to the metre with a different
pharmacy (a copied geocode or a neighbourhood centroid). Each doubtful position gets the best
alternative found in sources that are allowed to be reused: the AIRP registry, the other listings
of the same pharmacy in the directory, and OpenStreetMap (ODbL). Nothing is changed here:
`review_positions.csv` is checked by a person, then `load` applies it.
"""

from __future__ import annotations

import json
import math
import statistics
from collections import defaultdict
from urllib.parse import quote

from .common import OUT, STAGING, Fetcher, name_key, norm_coord, read_csv, write_csv
from .normalize import city_key, communes_named, distance_m, name_similarity, plus_code

REVIEW = OUT / "review_positions.csv"
DAY = 24 * 3600
# Abidjan communes as OpenStreetMap relations. Anyama, Bingerville and Songon have no boundary in
# Nominatim yet: their pharmacies are checked against the town instead.
COMMUNE_RELATIONS = {
    "Abobo": 3377895, "Adjamé": 3377835, "Attécoubé": 3377820, "Cocody": 3363950, "Koumassi": 3377741,
    "Marcory": 3308780, "Plateau": 3308306, "Port-Bouët": 3377873, "Treichville": 3308305, "Yopougon": 3377881,
}
COMMUNES_URL = ("https://nominatim.openstreetmap.org/lookup?format=geojson&polygon_geojson=1&osm_ids="
                + ",".join(f"R{r}" for r in COMMUNE_RELATIONS.values()))
OSM_PHARMACIES_URL = "https://overpass-api.de/api/interpreter?data=" + quote(
    '[out:json][timeout:120];nwr["amenity"="pharmacy"](4.0,-8.7,10.8,-2.4);out center tags;')
COMMUNE_TOLERANCE_M = 300  # boundaries are approximate and pharmacies sit on boundary roads
TOWN_MAX_KM = 15
SAME_PLACE_M = 300
MOVED_M = 200

FIELDS = [
    "ref", "name", "city", "commune", "address", "lat", "lng", "position_source", "issues",
    "proposed_source", "proposed_lat", "proposed_lng", "proposed_distance_m", "confirmed_by", "confidence",
    "accept", "corrected_lat", "corrected_lng", "note",
]


# --- Geometry ----------------------------------------------------------------------------

Polygon = list[list[tuple[float, float]]]  # rings of (lng, lat), outer ring first


def _in_ring(lng: float, lat: float, ring: list[tuple[float, float]]) -> bool:
    inside = False
    for (x1, y1), (x2, y2) in zip(ring, ring[1:] + ring[:1]):
        if (y1 > lat) != (y2 > lat) and lng < x1 + (lat - y1) * (x2 - x1) / (y2 - y1):
            inside = not inside
    return inside


def _segment_m(lat: float, lng: float, a: tuple[float, float], b: tuple[float, float]) -> float:
    """Distance to a segment, on a local flat projection (fine at commune scale)."""
    k = 111_320.0
    c = k * math.cos(math.radians(lat))
    px, py = 0.0, 0.0
    ax, ay = (a[0] - lng) * c, (a[1] - lat) * k
    bx, by = (b[0] - lng) * c, (b[1] - lat) * k
    dx, dy = bx - ax, by - ay
    t = 0.0 if dx == dy == 0 else max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)))
    return ((ax + t * dx - px) ** 2 + (ay + t * dy - py) ** 2) ** 0.5


def in_polygon(lat: float, lng: float, poly: Polygon, tolerance_m: float = 0) -> bool:
    outer, holes = poly[0], poly[1:]
    if _in_ring(lng, lat, outer) and not any(_in_ring(lng, lat, h) for h in holes):
        return True
    return tolerance_m > 0 and any(
        _segment_m(lat, lng, a, b) <= tolerance_m for ring in poly for a, b in zip(ring, ring[1:] + ring[:1]))


def parse_communes(geojson: str) -> dict[str, list[Polygon]]:
    by_relation = {v: k for k, v in COMMUNE_RELATIONS.items()}
    out: dict[str, list[Polygon]] = {}
    for f in json.loads(geojson)["features"]:
        name = by_relation.get(int(f["properties"]["osm_id"]))
        g = f["geometry"]
        polys = [g["coordinates"]] if g["type"] == "Polygon" else g["coordinates"] if g["type"] == "MultiPolygon" else []
        if name and polys:
            out[name] = [[[tuple(p) for p in ring] for ring in poly] for poly in polys]
    return out


def commune_at(lat: float, lng: float, communes: dict[str, list[Polygon]]) -> str | None:
    return next((c for c, polys in communes.items() if any(in_polygon(lat, lng, p) for p in polys)), None)


# --- Checks ------------------------------------------------------------------------------


def point(lat, lng) -> tuple[str, str]:
    """Key of a position at the database precision (6 decimals): the directory gives more."""
    return f"{float(lat):.6f}", f"{float(lng):.6f}"


class Checker:
    """Says what is wrong with a position for a given establishment.

    `airp` gives each establishment's own AIRP point. A point that appears once in the AIRP registry
    belongs to that pharmacy: the other pharmacies stored there are the copies.
    """

    def __init__(self, ests: list[dict], communes: dict[str, list[Polygon]], airp: dict[str, tuple[str, str]],
                 airp_count: dict[tuple[str, str], int]) -> None:
        self.communes = communes
        self.airp, self.airp_count = airp, airp_count
        pts: dict[str, list[tuple[float, float]]] = defaultdict(list)
        self.at: dict[tuple[str, str], list[dict]] = defaultdict(list)
        for e in ests:
            if e["kind"] == "pharmacie" and e["lat"] and e["lng"]:
                pts[city_key(e["city"])].append((float(e["lat"]), float(e["lng"])))
                self.at[point(e["lat"], e["lng"])].append(e)
        self.town = {c: (statistics.median(p[0] for p in v), statistics.median(p[1] for p in v))
                     for c, v in pts.items() if len(v) >= 3}

    def place_issue(self, e: dict, lat, lng) -> str | None:
        """Outside the commune of the address in Abidjan, far from the town elsewhere.

        The directory's location tag and the address sometimes name different communes: any of them is accepted.
        """
        lat, lng = float(lat), float(lng)
        expected = [c for c in dict.fromkeys([e["commune"], *communes_named(e["address"])]) if c in self.communes]
        if e["city"] == "Abidjan" and expected:
            if any(in_polygon(lat, lng, p, COMMUNE_TOLERANCE_M) for c in expected for p in self.communes[c]):
                return None
            found = commune_at(lat, lng, self.communes)
            return f"hors de {' ou '.join(expected)} ({'à ' + found if found else 'hors des communes de la carte'})"
        mid = self.town.get(city_key(e["city"]))
        if mid and (d := distance_m(lat, lng, *mid)) > TOWN_MAX_KM * 1000:
            return f"à {d / 1000:.0f} km du centre de {e['city']}"
        return None

    def owns(self, e: dict) -> bool:
        p = point(e["lat"], e["lng"])
        return self.airp.get(e["est_id"]) == p and self.airp_count.get(p) == 1

    def shared_with(self, e: dict) -> list[dict]:
        """Other pharmacies stored at exactly the same point, when they are not the same pharmacy listed twice."""
        if self.owns(e):
            return []
        others = [o for o in self.at.get(point(e["lat"], e["lng"]), []) if o is not e]
        return [o for o in others if name_similarity(o["name"], e["name"]) < 0.85]

    def issues(self, e: dict) -> list[str]:
        if not (e["lat"] and e["lng"]):
            return []
        out = []
        if p := self.place_issue(e, e["lat"], e["lng"]):
            out.append(p)
        if shared := self.shared_with(e):
            out.append(f"même position que {len(shared)} autre(s) pharmacie(s)")
        return out


def checker(ests: list[dict], links: list[dict], communes: dict[str, list[Polygon]]) -> Checker:
    airp = {r["airp_id"]: r for r in read_csv(STAGING / "airp_officines.csv")}
    own = {l["est_id"]: point(airp[l["source_record"]]["lat"], airp[l["source_record"]]["lng"])
           for l in links if l["source"] == "airp-officines" and airp.get(l["source_record"], {}).get("lat")}
    count: dict[tuple[str, str], int] = defaultdict(int)
    for a in airp.values():
        if a["lat"]:
            count[point(a["lat"], a["lng"])] += 1
    return Checker(ests, communes, own, count)


# --- Sources of positions ----------------------------------------------------------------


def _same(lat1, lng1, lat2, lng2, within_m: float = 1) -> bool:
    d = distance_m(lat1, lng1, lat2, lng2)
    return d is not None and d <= within_m


def candidates(e: dict, links: list[dict], airp: dict, site: dict, osm: list[dict]) -> list[tuple[str, str, str]]:
    """Every known position for this pharmacy, as (source, lat, lng), most reliable first."""
    out = []
    for l in links:
        if l["source"] == "airp-officines" and (a := airp.get(l["source_record"])) and a["lat"]:
            out.append(("AIRP", a["lat"], a["lng"]))
    for l in links:
        if l["source"] == "site-annuaire" and (s := site.get(l["source_record"])) and s["lat"]:
            out.append((f"annuaire (fiche {l['source_record']})", s["lat"], s["lng"]))
    key_city = city_key(e["city"])
    for o in osm:
        if name_similarity(o["name"], e["name"]) >= 0.85 and (not o["city"] or city_key(o["city"]) == key_city or e["city"] == "Abidjan"):
            out.append((f"OpenStreetMap ({o['id']})", o["lat"], o["lng"]))
    return out


def origin(e: dict, cands: list[tuple[str, str, str]], gmaps_refs: set[str]) -> str:
    if not e["lat"]:
        return ""
    for src, lat, lng in cands:
        if not src.startswith("OpenStreetMap") and _same(e["lat"], e["lng"], lat, lng):
            if src == "AIRP":
                return "AIRP"
            code = plus_code(e["address"], float(lat), float(lng))
            return "annuaire (plus code de l'adresse)" if code and _same(*code, lat, lng, 30) else "annuaire"
    return "Google Maps" if e["est_id"] in gmaps_refs else "inconnue"


def propose(e: dict, cands: list[tuple[str, str, str]], check: Checker, shared_points: set[tuple[str, str]]):
    """Best position that passes the checks, the sources that agree with it, and a confidence."""
    good = [(s, la, ln) for s, la, ln in cands
            if not check.place_issue(e, la, ln) and point(la, ln) not in shared_points
            and not (e["lat"] and _same(e["lat"], e["lng"], la, ln, MOVED_M))]
    if not good:
        return None, [], ""
    best = good[0]
    kind = lambda s: s.split(" (")[0]
    agree = sorted({kind(s) for s, la, ln in cands if kind(s) != kind(best[0]) and _same(best[1], best[2], la, ln, SAME_PLACE_M)})
    polygon_checked = e["city"] == "Abidjan" and any(c in check.communes for c in (e["commune"], *communes_named(e["address"])))
    confidence = "haute" if agree or (kind(best[0]) == "AIRP" and polygon_checked) else "moyenne"
    return best, agree, confidence


# --- OpenStreetMap -----------------------------------------------------------------------


def load_communes(f: Fetcher | None = None) -> dict[str, list[Polygon]]:
    f = f or Fetcher(delay_s=1.0)
    page = f.cached(COMMUNES_URL) or f.get(COMMUNES_URL)
    return parse_communes(page.text)


def load_osm_pharmacies(f: Fetcher) -> list[dict]:
    try:
        page = f.get(OSM_PHARMACIES_URL, max_age_s=30 * DAY)
        elements = json.loads(page.text)["elements"]
    except Exception as e:  # the public Overpass servers are often saturated: OSM is optional
        for path in f._paths(OSM_PHARMACIES_URL):  # an error page must not be reused from the cache
            path.unlink(missing_ok=True)
        print(f"OpenStreetMap indisponible ({e}) : propositions sans OSM")
        return []
    out = []
    for el in elements:
        tags = el.get("tags", {})
        lat = el.get("lat", el.get("center", {}).get("lat"))
        lng = el.get("lon", el.get("center", {}).get("lon"))
        if tags.get("name") and lat is not None:
            out.append({"id": f"{el['type']}/{el['id']}", "name": tags["name"], "city": tags.get("addr:city"),
                        "lat": norm_coord(lat, -90, 90), "lng": norm_coord(lng, -180, 180)})
    return out


# --- Step --------------------------------------------------------------------------------


def assess(ests: list[dict], links: list[dict], communes, osm: list[dict], gmaps_refs: set[str] = frozenset()) -> list[dict]:
    """One row per positioned pharmacy, with its issues and a proposed position when doubtful."""
    airp = {r["airp_id"]: r for r in read_csv(STAGING / "airp_officines.csv")}
    site = {r["listing_id"]: r for r in read_csv(STAGING / "site_directory.csv")}
    by_est = defaultdict(list)
    for l in links:
        by_est[l["est_id"]].append(l)
    by_osm_key = defaultdict(list)
    for o in osm:
        by_osm_key[name_key(o["name"]).split(" ")[0] if name_key(o["name"]) else ""].append(o)
    check = checker(ests, links, communes)
    # A point shared by different pharmacies, in our data or in the AIRP registry, is not a source either.
    shared_points = {p for p, es in check.at.items() if any(check.shared_with(x) for x in es)}
    shared_points |= {p for p, n in check.airp_count.items() if n > 1}

    rows = []
    for e in ests:
        if e["kind"] != "pharmacie" or not e["lat"]:
            continue
        key = name_key(e["name"])
        osm_near = by_osm_key.get(key.split(" ")[0] if key else "", [])
        cands = candidates(e, by_est[e["est_id"]], airp, site, osm_near)
        issues = check.issues(e)
        row = {"ref": e["est_id"], "name": e["name"], "city": e["city"], "commune": e["commune"],
               "address": e["address"], "lat": e["lat"], "lng": e["lng"],
               "position_source": origin(e, cands, gmaps_refs), "issues": " ; ".join(issues)}
        if issues:
            best, agree, confidence = propose(e, cands, check, shared_points)
            if best:
                row.update(proposed_source=best[0], proposed_lat=best[1], proposed_lng=best[2],
                           proposed_distance_m=round(distance_m(e["lat"], e["lng"], best[1], best[2])),
                           confirmed_by=", ".join(agree), confidence=confidence,
                           accept="oui" if confidence == "haute" else "")
            else:
                row["confidence"] = "aucune source"
        rows.append(row)
    return rows


def human_decision(r: dict) -> bool:
    """The row differs from what the pipeline prefills: someone has reviewed it."""
    prefilled = "oui" if r.get("confidence") == "haute" else ""
    return (r.get("accept") or "") != prefilled or any(r.get(k) for k in ("corrected_lat", "corrected_lng", "note"))


def summary(rows: list[dict]) -> dict[str, dict[str, int]]:
    """Counts by position source: positioned, outside commune or town, shared point, doubtful."""
    out: dict[str, dict[str, int]] = defaultdict(lambda: defaultdict(int))
    for r in rows:
        for src in (r["position_source"], "total"):
            c = out[src]
            c["positionnées"] += 1
            c["hors commune ou ville"] += "hors de" in r["issues"] or " km du centre" in r["issues"]
            c["position partagée"] += "même position" in r["issues"]
            c["douteuses"] += bool(r["issues"])
    return {k: dict(v) for k, v in out.items()}


def run() -> None:
    from .load import GMAPS_REVIEW, apply_gmaps

    ests = read_csv(OUT / "establishments.csv")
    links = read_csv(OUT / "establishment_links.csv")
    gmaps_refs = set()
    if GMAPS_REVIEW.exists():
        before = {e["est_id"]: e["lat"] for e in ests}
        apply_gmaps(ests, read_csv(GMAPS_REVIEW))
        gmaps_refs = {e["est_id"] for e in ests if e["lat"] != before[e["est_id"]]}
    f = Fetcher(delay_s=1.5)
    communes = load_communes(f)
    osm = load_osm_pharmacies(f)
    rows = assess(ests, links, communes, osm, gmaps_refs)

    # A new run keeps what a person decided, as long as the pharmacy and its position are the same.
    previous = {(r["ref"], r["name"]): r for r in read_csv(REVIEW)} if REVIEW.exists() else {}
    kept = 0
    for r in rows:
        old = previous.get((r["ref"], r["name"]))
        if old and human_decision(old) and (old["lat"], old["lng"]) == (r["lat"], r["lng"]):
            r.update(accept=old["accept"], corrected_lat=old.get("corrected_lat", ""),
                     corrected_lng=old.get("corrected_lng", ""), note=old.get("note", ""))
            kept += 1
    doubtful = [r for r in rows if r["issues"]]
    doubtful.sort(key=lambda r: ({"haute": 0, "moyenne": 1}.get(r.get("confidence"), 2), r["city"] or "", r["commune"] or "", r["name"]))
    write_csv(REVIEW, doubtful, FIELDS)

    print(f"positions : {len(communes)} communes d'Abidjan, {len(osm)} pharmacies OpenStreetMap")
    for src, c in sorted(summary(rows).items(), key=lambda x: -x[1]["positionnées"]):
        print(f"  {src} : " + ", ".join(f"{k} {v}" for k, v in c.items()))
    conf = defaultdict(int)
    for r in doubtful:
        conf[r.get("confidence") or "?"] += 1
    print(f"  à relire : {len(doubtful)} ({dict(conf)}) ; décisions reprises : {kept}")
    print(f"Relecture : {REVIEW}")
