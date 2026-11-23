"""pharmacies-de-garde.ci: weekly duty lists, drug price and CMU tables, establishment directory.

robots.txt allows crawling. Requests are spaced by `Fetcher.delay_s` and cached on disk.
The site is private and marked "All rights reserved": reuse in the product must be cleared.
"""

from __future__ import annotations

import html as htmllib
import json
import re
from datetime import date

from bs4 import BeautifulSoup

from .common import STAGING, Fetcher, clean, norm_coord, now_iso, parse_amount, split_phones, write_csv

BASE = "https://www.pharmacies-de-garde.ci"
SOURCE = "pharmacies-de-garde.ci"
DUTY_PAGES = {
    "abidjan": f"{BASE}/liste-des-pharmacies-de-garde-a-abidjan-votre-permanence/",
    "interieur": f"{BASE}/liste-des-pharmacies-de-garde-a-linterieur-votre-permanence/",
}
PRICES_PAGE = f"{BASE}/prix-des-medicaments-en-pharmacie-en-cote-divoire/"
CMU_PAGE = f"{BASE}/liste-des-medicaments-pris-en-charge-par-la-cmu/"
DAY = 24 * 3600
MONTHS = {m: i + 1 for i, m in enumerate(
    "janvier février mars avril mai juin juillet août septembre octobre novembre décembre".split())}


def _tables(page_html: str) -> list[list[list[str]]]:
    soup = BeautifulSoup(page_html, "lxml")
    return [
        [[clean(td.get_text(" ")) for td in tr.find_all(["td", "th"])] for tr in table.find_all("tr")]
        for table in soup.find_all("table")
    ]


def _modified(page_html: str) -> str | None:
    m = re.search(r'"dateModified":"([^"]+)"', page_html)
    return m.group(1) if m else None


def parse_period(page_html: str) -> tuple[date, date]:
    """'du Samedi 21 Novembre au Vendredi 27 Novembre 2026' -> (2026-11-21, 2026-11-27)."""
    text = clean(BeautifulSoup(page_html, "lxml").get_text(" "))
    m = re.search(r"du \w+ (\d{1,2}) (\w+)(?: (\d{4}))? au \w+ (\d{1,2}) (\w+) (\d{4})", text, re.I)
    if not m:
        raise ValueError("période de garde introuvable dans la page")
    d1, m1, y1, d2, m2, y2 = m.groups()
    end = date(int(y2), MONTHS[m2.lower()], int(d2))
    start_year = int(y1) if y1 else (end.year - 1 if MONTHS[m1.lower()] > end.month else end.year)
    return date(start_year, MONTHS[m1.lower()], int(d1)), end


def collect_duties(f: Fetcher) -> None:
    rows = []
    for zone, url in DUTY_PAGES.items():
        page = f.get(url, max_age_s=6 * 3600)
        start, end = parse_period(page.text)
        for table in _tables(page.text):
            for i, cells in enumerate(table[1:], start=2):
                if len(cells) < 3 or not cells[1]:
                    continue
                place, name, contact = cells[:3]
                commune, _, sector = (p.strip() for p in place.partition(" - "))
                pharmacist, _, phone_raw = contact.rpartition("TEL")
                rows.append({
                    "zone": zone,
                    "city": "ABIDJAN" if zone == "abidjan" else commune,
                    "commune": commune,
                    "sector": sector,
                    "pharmacy_name": name,
                    "pharmacist": clean(pharmacist.rstrip(" -")) or None,
                    "phone_raw": clean(phone_raw.lstrip(".: ")),
                    "phones": " ".join(split_phones(phone_raw)),
                    "week_start": start.isoformat(),
                    "week_end": end.isoformat(),
                    "hours_note": "Garde de 20h à 8h en semaine et le week-end (texte de la page)",
                    "source_url": url,
                    "source_row": i,
                    "source_modified": _modified(page.text),
                    "fetched_at": page.fetched_at,
                })
    path = write_csv(STAGING / "site_duties.csv", rows, list(rows[0]))
    print(f"{path.name}: {len(rows)} gardes")


def collect_price_tables(f: Fetcher) -> None:
    page = f.get(PRICES_PAGE, max_age_s=7 * DAY)
    rows = []
    for table in _tables(page.text):
        for i, c in enumerate(table[1:], start=2):
            if len(c) < 5 or not c[1]:
                continue
            rows.append({
                "code": c[1], "label": c[2], "therapeutic_group": c[3],
                "price_raw": c[4], "price_fcfa": parse_amount(c[4]),
                "source_url": PRICES_PAGE, "source_row": i,
                "source_modified": _modified(page.text), "fetched_at": page.fetched_at,
            })
    write_csv(STAGING / "site_prices.csv", rows, list(rows[0]))
    print(f"site_prices.csv: {len(rows)} lignes")

    page = f.get(CMU_PAGE, max_age_s=7 * DAY)
    rows = []
    for table in _tables(page.text):
        for i, c in enumerate(table[1:], start=2):
            if len(c) < 6 or not c[1]:
                continue
            rows.append({
                "label": c[1], "price_raw": c[2], "price_fcfa": parse_amount(c[2]),
                "ingredient": c[3], "therapeutic_group": c[4], "dosage_form": c[5],
                "source_url": CMU_PAGE, "source_row": i,
                "source_modified": _modified(page.text), "fetched_at": page.fetched_at,
            })
    write_csv(STAGING / "site_cmu.csv", rows, list(rows[0]))
    print(f"site_cmu.csv: {len(rows)} lignes")


def _terms(f: Fetcher, taxonomy: str) -> dict[int, dict]:
    terms: dict[int, dict] = {}
    page = 1
    while True:
        p = f.get(f"{BASE}/wp-json/wp/v2/{taxonomy}?per_page=100&page={page}&_fields=id,name,slug,parent", max_age_s=7 * DAY)
        batch = json.loads(p.text)
        if not batch:
            break
        terms.update({t["id"]: t for t in batch})
        if len(batch) < 100:
            break
        page += 1
    return terms


def _listing_index(f: Fetcher) -> list[dict]:
    items: list[dict] = []
    page = 1
    fields = "id,slug,link,title,modified_gmt,listing-category,listing-locations,listing-tag"
    while True:
        p = f.get(f"{BASE}/wp-json/wp/v2/listing?per_page=100&page={page}&orderby=id&order=asc&_fields={fields}", max_age_s=7 * DAY)
        batch = json.loads(p.text)
        if not isinstance(batch, list) or not batch:
            break
        items.extend(batch)
        if len(batch) < 100:
            break
        page += 1
    return items


_DAYS = {"Mon": "lun", "Tue": "mar", "Wed": "mer", "Thu": "jeu", "Fri": "ven", "Sat": "sam", "Sun": "dim"}


def _to_24h(t: str) -> str | None:
    m = re.fullmatch(r"(\d{1,2}):(\d{2}) ?([AP]M)", t.strip(), re.I)
    if not m:
        return None
    h, mi, ap = int(m.group(1)), m.group(2), m.group(3).upper()
    h = h % 12 + (12 if ap == "PM" else 0)
    return f"{h:02d}:{mi}"


def parse_listing(page_html: str) -> dict:
    soup = BeautifulSoup(page_html, "lxml")
    out: dict = {"lat": None, "lng": None, "address": None, "phone_raw": None, "hours": None, "description": None}
    iframe = soup.select_one(".box-map iframe")
    if iframe and (m := re.search(r"q=(-?\d+\.\d+),\s*(-?\d+\.\d+)", iframe.get("src", ""))):
        out["lat"], out["lng"] = m.group(1), m.group(2)
    for li in soup.select("ul.ul-disc li"):
        text = clean(li.get_text(" "))
        if text.lower().startswith(("téléphone", "telephone")):
            out["phone_raw"] = clean(text.split(":", 1)[-1])
        elif out["address"] is None and text:
            out["address"] = text
    hours = {}
    for row in soup.select("div.row.mb-1"):
        spans = row.find_all("span")
        if len(spans) >= 2 and (day := clean(spans[0].get_text())) in _DAYS:
            value = clean(spans[1].get_text(" "))
            parts = [p for p in re.split(r"\s*-\s*", value) if p]
            if len(parts) == 2 and all(_to_24h(p) for p in parts):
                hours[_DAYS[day]] = f"{_to_24h(parts[0])}-{_to_24h(parts[1])}"
            else:
                hours[_DAYS[day]] = value.lower()
    out["hours"] = json.dumps(hours, ensure_ascii=False) if hours else None
    desc = soup.select_one(".entry-content, .listing-content, .description")
    if desc:
        out["description"] = clean(desc.get_text(" "))[:600] or None
    return out


def _repair_quotes(raw: str) -> str:
    """The site writes titles like 'Clinique "La Providence"' without escaping the quotes."""
    def fix(m: re.Match) -> str:
        return f'"{m.group(1)}":"{m.group(2).replace(chr(92) + chr(34), chr(34)).replace(chr(34), chr(92) + chr(34))}","{m.group(3)}"'
    return re.sub(r'"(title|address|locations)":"(.*?)","(dlink|image|address|locations|lat)"', fix, raw)


def _markers(f: Fetcher) -> dict[str, dict]:
    """Map markers embedded in the directory pages: name, address and GPS of every listing, 40 per page."""
    markers: dict[str, dict] = {}
    page_no, last = 1, 1
    while page_no <= last:
        url = f"{BASE}/listing/" if page_no == 1 else f"{BASE}/listing/page/{page_no}/"
        page = f.get(url, max_age_s=7 * DAY)
        last = max([last, *map(int, re.findall(r"/listing/page/(\d+)/", page.text))])
        for script in BeautifulSoup(page.text, "lxml").find_all("script"):
            text = script.string or ""
            if "dirs_json" in text:
                obj = json.loads(text[text.index("{"): text.rindex("}") + 1])
                if not isinstance(obj.get("dirs_json"), str):
                    continue
                try:
                    items = json.loads(_repair_quotes(obj["dirs_json"]))
                except json.JSONDecodeError as e:
                    print(f"  marqueurs illisibles page {page_no} : {e}", flush=True)
                    continue
                for d in items:
                    markers[d["dlink"]] = {**d, "fetched_at": page.fetched_at}
        page_no += 1
    return markers


def collect_directory(f: Fetcher, limit: int | None = None) -> None:
    """Every listing with its GPS from the map markers, plus phone and hours from detail pages already cached."""
    cats = _terms(f, "listing-category")
    locs = _terms(f, "listing-locations")
    index = _listing_index(f)
    markers = _markers(f)
    rows, with_details = [], 0
    for item in index[:limit] if limit else index:
        mk = markers.get(item["link"], {})
        cached = f.cached(item["link"])
        d = parse_listing(cached.text) if cached else {}
        with_details += bool(cached)
        rows.append({
            "listing_id": item["id"],
            "name": clean(htmllib.unescape(item["title"]["rendered"])),
            "categories": " | ".join(cats[i]["name"] for i in item.get("listing-category", []) if i in cats),
            "locations": " | ".join(locs[i]["name"] for i in item.get("listing-locations", []) if i in locs),
            "address": d.get("address") or clean(mk.get("address")) or None,
            "lat": norm_coord(d.get("lat") or mk.get("lat"), -90, 90),
            "lng": norm_coord(d.get("lng") or mk.get("lng"), -180, 180),
            "phone_raw": d.get("phone_raw"),
            "phones": " ".join(split_phones(d.get("phone_raw"))),
            "hours": d.get("hours"),
            "description": d.get("description"),
            "details_fetched": bool(cached),
            "source_url": item["link"],
            "source_modified": item.get("modified_gmt"),
            "fetched_at": cached.fetched_at if cached else mk.get("fetched_at"),
        })
    write_csv(STAGING / "site_directory.csv", rows, list(rows[0]))
    print(f"site_directory.csv : {len(rows)} fiches, {sum(1 for r in rows if r['lat'])} avec GPS, "
          f"{with_details} avec fiche détaillée (téléphone, horaires)")


def crawl_details(f: Fetcher, limit: int | None = None) -> None:
    """Fetch every detail page into the cache (about 6 s each). Interrupt and resume at will."""
    index = _listing_index(f)
    todo = [i for i in index if f.cached(i["link"]) is None]
    print(f"fiches détaillées : {len(index) - len(todo)} déjà en cache, {len(todo)} à récupérer", flush=True)
    for n, item in enumerate(todo[:limit] if limit else todo, start=1):
        try:
            f.get(item["link"])
        except Exception as e:  # one broken page must not stop the crawl
            print(f"  échec {item['link']} : {e}", flush=True)
        if n % 100 == 0:
            print(f"  {n}/{len(todo)}", flush=True)


def run(only: str | None = None, limit: int | None = None) -> None:
    f = Fetcher(delay_s=1.5)
    started = now_iso()
    if only in (None, "duties"):
        collect_duties(f)
    if only in (None, "tables"):
        collect_price_tables(f)
    if only == "details":
        crawl_details(f, limit=limit)
    if only in (None, "directory"):
        collect_directory(f, limit=limit)
    print(f"{SOURCE} : collecte démarrée à {started}")
