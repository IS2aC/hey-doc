"""Deduplication of products and establishments across sources.

Every merge keeps a link to each source record (method and score), and every uncertain
case goes to a review file instead of being merged.
"""

from __future__ import annotations

import re
from collections import defaultdict

from .common import OUT, STAGING, clean, name_key, read_csv, split_phones, write_csv
from .normalize import city_key, commune_of, distance_m, label_features, label_key, label_words, name_similarity, plain

# --- Products --------------------------------------------------------------------------


class Products:
    def __init__(self) -> None:
        self.items: list[dict] = []
        self.refs: list[dict] = []
        self.prices: list[dict] = []
        self.coverage: list[dict] = []
        self.review: list[dict] = []
        self.by_code: dict[str, dict] = {}
        self.by_key: dict[str, list[dict]] = defaultdict(list)

    def add(self, label: str, origin: str, **attrs) -> dict:
        f = label_features(label)
        p = {
            "pres_id": f"P{len(self.items) + 1:05d}",
            "label": clean(label),
            "ingredient": attrs.get("ingredient") or None,
            "strength": " + ".join(f["strengths"]) or None,
            "dosage_form": attrs.get("dosage_form") or f["form"],
            "pack": f["pack"],
            "therapeutic_group": attrs.get("therapeutic_group") or None,
            "product_type": {"G": "générique", "S": "spécialité"}.get(attrs.get("type", ""), None),
            "origin": origin,
            "review": attrs.get("review") or None,
        }
        self.items.append(p)
        self.by_key[label_key(label)].append(p)
        return p

    def ref(self, p: dict, source: str, record: str, label: str, method: str, score: float | None = None,
            code: str | None = None, page: str | None = None) -> None:
        self.refs.append({"pres_id": p["pres_id"], "source": source, "source_code": code, "source_record": record,
                          "original_label": label, "page": page, "match_method": method,
                          "match_score": None if score is None else round(score, 3)})

    def find(self, label: str, ingredient: str | None) -> tuple[dict | None, str, float]:
        """Match an edition written differently: same brand and same numbers, then the ingredient breaks ties."""
        cands = self.by_key.get(label_key(label), [])
        if len(cands) == 1:
            return cands[0], "marque+nombres", 1.0
        if len(cands) > 1 and ingredient:
            same = [c for c in cands if plain(c["ingredient"]) == plain(ingredient)]
            if len(same) == 1:
                return same[0], "marque+nombres+dci", 1.0
        if len(cands) > 1:
            words = label_words(label)
            scored = sorted(((len(words & label_words(c["label"])) / max(1, len(words | label_words(c["label"]))), c)
                             for c in cands), key=lambda x: -x[0])
            if scored[0][0] > scored[1][0]:
                return scored[0][1], "marque+nombres+mots", scored[0][0]
            return None, f"ambigu ({len(cands)} candidats)", 0.0
        return None, "aucun candidat", 0.0


def dedup_products() -> None:
    P = Products()
    mugef = read_csv(STAGING / "mugef.csv")

    for r in (r for r in mugef if r["edition"] == "2026-09"):
        p = P.add(r["label"], "MUGEF-CI 2026-09", ingredient=r["ingredient"], therapeutic_group=r["therapeutic_group"],
                  type=r["type"], review=r["review"])
        P.by_code[r["code"]] = p
        P.ref(p, "mugef-2026-09", f"code:{r['code']}", r["label"], "code", 1.0, r["code"], r["page"])
        P.prices.append({"pres_id": p["pres_id"], "source": "mugef-2026-09", "kind": "mugefci_published",
                         "amount_fcfa": r["price_fcfa"], "observed_on": "2026-09-08"})
        P.coverage.append({"pres_id": p["pres_id"], "source": "mugef-2026-09", "insurer": "MUGEF-CI",
                           "scheme": r["scheme"], "category": r["category"]})

    for r in read_csv(STAGING / "site_prices.csv"):
        p = P.by_code.get(r["code"])
        method = "code"
        if p is None:
            p = P.add(r["label"], "pharmacies-de-garde.ci prix", therapeutic_group=r["therapeutic_group"],
                      review="absent de la liste MUGEF-CI 2026")
            P.by_code[r["code"]] = p
            method = "nouvelle présentation"
        P.ref(p, "site-prix", f"code:{r['code']}", r["label"], method, 1.0, r["code"])
        if r["price_fcfa"]:
            P.prices.append({"pres_id": p["pres_id"], "source": "site-prix", "kind": "mugefci_published",
                             "amount_fcfa": r["price_fcfa"], "observed_on": (r["source_modified"] or "")[:10]})

    by_label_2022: dict[str, dict] = {}
    for r in (r for r in mugef if r["edition"] == "2022-11"):
        p, method, score = P.find(r["label"], r["ingredient"])
        if p is None:
            P.review.append({"source": "mugef-2022-11", "record": r["code"], "label": r["label"], "reason": method})
            p = P.add(r["label"], "MUGEF-CI 2022-11", ingredient=r["ingredient"], therapeutic_group=r["therapeutic_group"],
                      review=f"non rapprochée de 2026 : {method}")
            method = "nouvelle présentation"
        by_label_2022[plain(r["label"])] = p
        P.ref(p, "mugef-2022-11", f"code:{r['code']}", r["label"], method, score, r["code"], r["page"])
        P.prices.append({"pres_id": p["pres_id"], "source": "mugef-2022-11", "kind": "mugefci_published",
                         "amount_fcfa": r["price_fcfa"], "observed_on": "2022-11-10"})
        P.coverage.append({"pres_id": p["pres_id"], "source": "mugef-2022-11", "insurer": "MUGEF-CI",
                           "scheme": r["scheme"], "category": r["category"]})

    for r in read_csv(STAGING / "site_cmu.csv"):
        p = by_label_2022.get(plain(r["label"]))
        method, score = "libellé identique à 2022", 1.0
        if p is None:
            p, method, score = P.find(r["label"], r["ingredient"])
        if p is None:
            P.review.append({"source": "site-cmu", "record": r["source_row"], "label": r["label"], "reason": method})
            p = P.add(r["label"], "pharmacies-de-garde.ci CMU", ingredient=r["ingredient"],
                      therapeutic_group=r["therapeutic_group"], dosage_form=r["dosage_form"],
                      review=f"non rapprochée : {method}")
            method = "nouvelle présentation"
        elif r["dosage_form"]:
            p["dosage_form"] = r["dosage_form"]
        p["ingredient"] = p["ingredient"] or r["ingredient"] or None
        P.ref(p, "site-cmu", f"ligne:{r['source_row']}", r["label"], method, score)

    for r in read_csv(STAGING / "cmu_hta.csv"):
        ingredient = clean(re.split(r"\s\d", r["molecule_heading"], maxsplit=1)[0]) or None
        p, method, score = P.find(r["label"], ingredient)
        if p is None:
            P.review.append({"source": "cmu-hta-2025", "record": f"{r['list'][-30:]}#{r['row_number']}",
                             "label": r["label"], "reason": method})
            p = P.add(r["label"], "Décision CMU 0016 du 29/09/2025", ingredient=ingredient,
                      review=f"non rapprochée : {method}" + (f" ; OCR : {r['review']}" if r["review"] else ""))
            method = "nouvelle présentation"
        p["ingredient"] = p["ingredient"] or ingredient
        P.ref(p, "cmu-hta-2025", f"{'HTA' if 'HYPERT' in r['list'] else 'DIABETE'}#{r['row_number']}",
              r["label"], method, score, page=r["page"])
        if r["price_fcfa"]:
            P.prices.append({"pres_id": p["pres_id"], "source": "cmu-hta-2025", "kind": "cmu_contracted",
                             "amount_fcfa": r["price_fcfa"], "observed_on": "2025-09-29",
                             "review": r["review"] or None})
        P.coverage.append({"pres_id": p["pres_id"], "source": "cmu-hta-2025", "insurer": "CNAM",
                           "scheme": "CMU", "category": "HTA" if "HYPERT" in r["list"] else "DIABETE"})

    write_csv(OUT / "presentations.csv", P.items, list(P.items[0]))
    write_csv(OUT / "presentation_refs.csv", P.refs, list(P.refs[0]))
    price_fields = ["pres_id", "source", "kind", "amount_fcfa", "observed_on", "review"]
    write_csv(OUT / "product_prices.csv", [{k: x.get(k) for k in price_fields} for x in P.prices], price_fields)
    write_csv(OUT / "product_coverage.csv", P.coverage, list(P.coverage[0]))
    write_csv(OUT / "review_products.csv", P.review or [{"source": "", "record": "", "label": "", "reason": ""}],
              ["source", "record", "label", "reason"])
    by_source = defaultdict(lambda: defaultdict(int))
    for r in P.refs:
        by_source[r["source"]]["nouvelle" if r["match_method"] == "nouvelle présentation" else "rapprochée"] += 1
    print(f"produits : {len(P.items)} présentations distinctes, {len(P.prices)} prix, {len(P.coverage)} références de couverture")
    for s, c in by_source.items():
        print(f"  {s} : {dict(c)}")


# --- Establishments --------------------------------------------------------------------

NON_HEALTH = {"école", "marché", "maison hantée", "institut de beauté", "institut de spa et bien-être",
              "esthéticien", "résidence", "programme d'alphabétisation", "service de blanchiment des dents",
              "clinique vétérinaire", "magasin d'articles dentaires", "centre de bien-être"}
KIND_RULES = [
    ("pharmacie", "pharmacie"),
    ("hôpital", "hôpital"),
    ("chu", "hôpital"),
    ("clinique", "clinique"),
    ("laboratoire", "laboratoire"),
    ("centre de santé", "centre de santé"),
    ("dispensaire", "centre de santé"),
    ("maison de santé", "centre de santé"),
    ("centre médical", "centre de santé"),
    ("centre de soins", "centre de santé"),
    ("radiolog", "imagerie"),
    ("imagerie", "imagerie"),
    ("irm", "imagerie"),
    ("dentist", "dentaire"),
    ("dentaire", "dentaire"),
    ("orthodont", "dentaire"),
    ("opticien", "optique"),
    ("ophtalm", "ophtalmologie"),
    ("cabinet", "cabinet médical"),
    ("médecin", "cabinet médical"),
    ("infirm", "soins infirmiers"),
    ("sage-femme", "maternité"),
    ("maternité", "maternité"),
    ("kinésithérap", "rééducation"),
    ("rééducation", "rééducation"),
]


def kind_of(categories: str, name: str) -> str | None:
    cats = [c.strip().lower() for c in categories.split("|") if c.strip()]
    if cats and all(c in NON_HEALTH for c in cats):
        return None
    text = " ".join(cats) + " " + name.lower()
    if re.search(r"\bpharmac", name.lower()):
        return "pharmacie"
    for needle, kind in KIND_RULES:
        if needle in text:
            return kind
    return "autre établissement de santé"


def _flag_airp_coords(airp: list[dict]) -> None:
    """AIRP points more than 30 km from the median of their own town are marked doubtful."""
    by_city: dict[str, list[dict]] = defaultdict(list)
    for a in airp:
        if a["lat"]:
            by_city[city_key(a["city"])].append(a)
    for rows in by_city.values():
        lats = sorted(float(a["lat"]) for a in rows)
        lngs = sorted(float(a["lng"]) for a in rows)
        mlat, mlng = lats[len(lats) // 2], lngs[len(lngs) // 2]
        for a in rows:
            d = distance_m(a["lat"], a["lng"], mlat, mlng)
            a["_coords_ok"] = len(rows) < 3 or d < 30000
    for a in airp:
        a.setdefault("_coords_ok", False)


def _best_unique(cands: list[tuple[float, dict]], margin: float = 0.05) -> tuple[dict | None, float, bool]:
    """Best candidate if it beats the runner-up by `margin`; the flag says whether there was a tie."""
    if not cands:
        return None, 0.0, False
    cands = sorted(cands, key=lambda c: -c[0])
    if len(cands) == 1 or cands[0][0] - cands[1][0] >= margin:
        return cands[0][1], cands[0][0], False
    return None, cands[0][0], True


def _airp_match(site: dict, airp: list[dict], airp_by_city: dict[str, list[dict]]):
    """Same town and similar name first; GPS only breaks ties or confirms a weaker name."""
    cands = []
    pool = {id(a): a for a in airp_by_city.get(city_key(site["city"]), [])}
    for a in airp:
        if a["_coords_ok"]:
            d = distance_m(site["lat"], site["lng"], a["lat"], a["lng"])
            if d is not None and d < 500:
                pool[id(a)] = a
    for a in pool.values():
        s = name_similarity(site["name"], a["name"])
        d = distance_m(site["lat"], site["lng"], a["lat"], a["lng"]) if a["_coords_ok"] else None
        same_city = city_key(a["city"]) == city_key(site["city"])
        if not ((s >= 0.85 and same_city) or (s >= 0.7 and d is not None and d < 300) or (s >= 0.55 and d is not None and d < 60)):
            continue
        score = s + (0.1 if d is not None and d < 500 else 0) + (0.05 if site.get("commune") and site["commune"] == commune_of(a["address"]) else 0)
        cands.append((score, a))
    a, score, tie = _best_unique(cands)
    if a is None:
        return None, "ambigu" if tie else "", 0.0, None
    d = distance_m(site["lat"], site["lng"], a["lat"], a["lng"])
    method = "nom+gps" if d is not None and d < 500 and a["_coords_ok"] else "nom+ville"
    return a, method, min(score, 1.0), d


def dedup_establishments() -> None:
    airp = read_csv(STAGING / "airp_officines.csv")
    site = read_csv(STAGING / "site_directory.csv")
    duties = read_csv(STAGING / "site_duties.csv")

    ents: list[dict] = []
    links: list[dict] = []
    review: list[dict] = []

    def new(kind: str, name: str, **kw) -> dict:
        e = {"est_id": f"E{len(ents) + 1:05d}", "kind": kind, "name": clean(name), "address": None, "city": None,
             "commune": None, "lat": None, "lng": None, "phones": [], "opening_hours": None,
             "airp_authorization": None, "license_status": None, "license_valid_until": None,
             "pharmacist": None, "categories": None, "description": None, "flags": []}
        e.update(kw)
        ents.append(e)
        return e

    def link(e: dict, source: str, record: str, method: str, score: float | None = None, dist: float | None = None) -> None:
        links.append({"est_id": e["est_id"], "source": source, "source_record": record, "match_method": method,
                      "match_score": None if score is None else round(score, 3),
                      "distance_m": None if dist is None else round(dist)})

    for a in airp:
        city = a["city"]
        commune = commune_of(a["address"]) if city_key(city) == "abidjan" else None
        e = new("pharmacie", a["name"], address=a["address"], city=city, commune=commune, lat=a["lat"] or None,
                lng=a["lng"] or None, airp_authorization=a["authorization_number"], license_status=a["license_status"],
                license_valid_until=a["validity_end"] or None)
        e["_airp"] = a
        a["_ent"] = e
        link(e, "airp-officines", a["airp_id"], "registre (référence)", 1.0)
    airp_by_city: dict[str, list[dict]] = defaultdict(list)
    for a in airp:
        airp_by_city[city_key(a["city"])].append(a)
    _flag_airp_coords(airp)
    for a in airp:
        if a["lat"] and not a["_coords_ok"]:
            a["_ent"]["flags"].append("coordonnées AIRP éloignées de la ville déclarée")

    # Site directory: pharmacies are attached to the AIRP registry, other establishments are deduplicated together.
    claimed: dict[str, tuple[float, dict]] = {}
    pending: list[tuple[dict, str]] = []
    skipped_non_health = 0
    for s in site:
        if re.match(r"(?i)pharmacies de garde", s["name"]):
            continue
        kind = kind_of(s["categories"], s["name"])
        if kind is None:
            skipped_non_health += 1
            continue
        locs = [l.strip() for l in s["locations"].split("|") if l.strip()]
        s["city"] = "Abidjan" if any(commune_of(l) or plain(l) == "abidjan" for l in locs) else (locs[0] if locs else None)
        s["commune"] = commune_of(*locs, s["address"]) if s["city"] == "Abidjan" else None
        if kind == "pharmacie":
            a, method, score, dist = _airp_match(s, airp, airp_by_city)
            if a is None and method == "ambigu":
                review.append({"kind": "fiche annuaire ambiguë", "est_id": "", "name": s["name"], "city": s["city"],
                               "detail": f"fiche site {s['listing_id']}"})
            if a is not None:
                # The site often lists the same pharmacy several times: all copies join the AIRP record,
                # the best-scored one supplies the details.
                s["_match"] = (a, method, score, dist)
                claimed.setdefault(a["airp_id"], []).append((score, s))
                continue
            pending.append((s, kind))
        else:
            pending.append((s, kind))

    for airp_id, copies in claimed.items():
        copies.sort(key=lambda c: (-c[0], not c[1]["details_fetched"] == "True"))
        for _, dup in copies[1:]:
            a, method, score, dist = dup["_match"]
            link(a["_ent"], "site-annuaire", dup["listing_id"], f"doublon de fiche ({method})", score, dist)
            a["_ent"]["phones"] = a["_ent"]["phones"] or split_phones(dup["phone_raw"])
            a["_ent"]["opening_hours"] = a["_ent"]["opening_hours"] or dup["hours"] or None
        score, s = copies[0]
        a, method, score, dist = s["_match"]
        e = a["_ent"]
        e["phones"] = split_phones(s["phone_raw"]) or e["phones"]
        e["opening_hours"] = s["hours"] or None
        e["commune"] = e["commune"] or s["commune"]
        e["description"] = s["description"] or None
        e["categories"] = s["categories"]
        if s["lat"] and (e["lat"] is None or not a["_coords_ok"] or (dist or 0) > 2000):
            if e["lat"] and (dist or 0) > 2000:
                e["flags"].append(f"position AIRP à {dist / 1000:.1f} km de la fiche annuaire, position annuaire retenue")
            e["lat"], e["lng"] = s["lat"], s["lng"]
        link(e, "site-annuaire", s["listing_id"], method, score, dist)

    seen: dict[str, list[dict]] = defaultdict(list)
    for s, kind in pending:
        key = (name_key(s["name"]), city_key(s["city"]))
        twin = next((e for e in seen[key]
                     if (d := distance_m(s["lat"], s["lng"], e["lat"], e["lng"])) is None or d < 1000), None)
        if twin:
            link(twin, "site-annuaire", s["listing_id"], "doublon de fiche (même nom, même ville, <1 km)", 1.0,
                 distance_m(s["lat"], s["lng"], twin["lat"], twin["lng"]))
            twin["phones"] = twin["phones"] or split_phones(s["phone_raw"])
            continue
        e = new(kind, s["name"], address=s["address"], city=s["city"], commune=s["commune"], lat=s["lat"] or None,
                lng=s["lng"] or None, phones=split_phones(s["phone_raw"]), opening_hours=s["hours"] or None,
                categories=s["categories"], description=s["description"] or None)
        if kind == "pharmacie":
            e["flags"].append("absente du registre AIRP des officines agréées")
            review.append({"kind": "pharmacie hors registre AIRP", "est_id": e["est_id"], "name": e["name"],
                           "city": e["city"], "detail": f"fiche site {s['listing_id']}"})
        seen[key].append(e)
        link(e, "site-annuaire", s["listing_id"], "nouvel établissement")

    # Weekly duties: phone first, then name within the same city and commune.
    by_phone: dict[str, list[dict]] = defaultdict(list)
    for e in ents:
        for p in e["phones"]:
            by_phone[p].append(e)
    pharmacies = [e for e in ents if e["kind"] == "pharmacie"]
    duty_rows = []
    for d in duties:
        city = "Abidjan" if d["zone"] == "abidjan" else d["city"].title()
        commune = commune_of(d["commune"], d["sector"]) if d["zone"] == "abidjan" else None
        match, method, score = None, "", 0.0
        phone_hits = {e["est_id"]: e for p in d["phones"].split() for e in by_phone.get(p, []) if e["kind"] == "pharmacie"}
        if len(phone_hits) == 1:
            match, method, score = next(iter(phone_hits.values())), "téléphone", 1.0
        else:
            cands = []
            for e in pharmacies:
                if city_key(e["city"]) != city_key(city):
                    continue
                sim = name_similarity(d["pharmacy_name"], e["name"])
                if sim < 0.8:
                    continue
                if commune and e["commune"]:
                    sim += 0.1 if e["commune"] == commune else -0.2
                if e["airp_authorization"]:
                    sim += 0.02
                cands.append((sim, e))
            best, score, tie = _best_unique([c for c in cands if c[0] >= 0.8])
            if best is not None:
                match, method = best, "nom+ville" + ("+commune" if commune and best["commune"] == commune else "")
            elif tie:
                review.append({"kind": "garde ambiguë", "est_id": " ".join(c[1]["est_id"] for c in sorted(cands, key=lambda c: -c[0])[:4]),
                               "name": d["pharmacy_name"], "city": city, "detail": d["commune"]})
        if match is None:
            match = new("pharmacie", d["pharmacy_name"].title(), city=city, commune=commune,
                        phones=d["phones"].split(), flags=["connue seulement par la liste de garde"])
            method = "nouvel établissement"
            review.append({"kind": "garde sans établissement connu", "est_id": match["est_id"],
                           "name": d["pharmacy_name"], "city": city, "detail": d["commune"]})
        match["pharmacist"] = d["pharmacist"] or match["pharmacist"]
        for p in d["phones"].split():
            if p not in match["phones"]:
                match["phones"].append(p)
        match["commune"] = match["commune"] or commune
        link(match, "site-gardes", f"{d['zone']}#{d['source_row']}", method, score)
        duty_rows.append({"est_id": match["est_id"], "zone": d["zone"], "city": city, "commune": commune,
                          "duty_area": " - ".join(x for x in (d["commune"], d["sector"]) if x),
                          "week_start": d["week_start"], "week_end": d["week_end"], "match_method": method,
                          "source_url": d["source_url"], "source_row": d["source_row"]})

    fields = ["est_id", "kind", "name", "address", "city", "commune", "lat", "lng", "phones", "opening_hours",
              "airp_authorization", "license_status", "license_valid_until", "pharmacist", "categories",
              "description", "flags"]
    rows = [{**{k: e[k] for k in fields}, "phones": " ".join(e["phones"]), "flags": " ; ".join(e["flags"])} for e in ents]
    write_csv(OUT / "establishments.csv", rows, fields)
    write_csv(OUT / "establishment_links.csv", links, list(links[0]))
    write_csv(OUT / "duties.csv", duty_rows, list(duty_rows[0]))
    write_csv(OUT / "review_establishments.csv", review or [{"kind": "", "est_id": "", "name": "", "city": "", "detail": ""}],
              ["kind", "est_id", "name", "city", "detail"])
    kinds = defaultdict(int)
    for e in ents:
        kinds[e["kind"]] += 1
    methods = defaultdict(int)
    for r in duty_rows:
        methods[r["match_method"]] += 1
    print(f"établissements : {len(ents)} distincts depuis {len(airp)} AIRP + {len(site)} fiches site + {len(duties)} gardes")
    print(f"  par type : {dict(sorted(kinds.items(), key=lambda x: -x[1]))}")
    print(f"  officines AIRP retrouvées sur le site : {len(claimed)} (avec {sum(len(c) for c in claimed.values())} fiches) ; "
          f"hors santé ignorées : {skipped_non_health}")
    print(f"  gardes rattachées : {dict(methods)} ; cas à relire : {len(review)}")
    print(f"  liens de provenance : {len(links)}")


def run() -> None:
    dedup_products()
    if (STAGING / "site_directory.csv").exists():
        dedup_establishments()
    else:
        print("établissements : annuaire du site absent, lancer d'abord l'étape site")
