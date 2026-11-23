"""AIRP public registry (sigir.airp.ci): licensed pharmacies, plus marketing authorizations and
quality alerts. The last two endpoints return no records yet; they are saved anyway so that
the pipeline picks them up as soon as AIRP publishes them.
"""

from __future__ import annotations

import json

from .common import RAW, STAGING, Fetcher, clean, norm_coord, write_csv

API = "https://sigir.airp.ci/api/public"
HOUR = 3600


def run() -> None:
    f = Fetcher(delay_s=1.0)
    page = f.get(f"{API}/officines", max_age_s=24 * HOUR)
    data = json.loads(page.text)
    rows = []
    for o in data["data"]:
        rows.append({
            "airp_id": o["id"],
            "name": clean(o.get("name")),
            "address": clean(o.get("address")) or None,
            "region": clean(o.get("region")) or None,
            "city": clean(o.get("city")) or None,
            "lat": norm_coord(o.get("lat"), -90, 90) if o.get("has_coordinates") else None,
            "lng": norm_coord(o.get("lng"), -180, 180) if o.get("has_coordinates") else None,
            "telephone": o.get("telephone"),
            "authorization_number": o.get("authorization_number"),
            "license_status": o.get("license_status"),
            "validity_end": o.get("validity_end"),
            "suspended_since": o.get("suspended_since"),
            "public_measure": o.get("public_measure"),
            "source_url": f"{API}/officines",
            "fetched_at": page.fetched_at,
        })
    write_csv(STAGING / "airp_officines.csv", rows, list(rows[0]))
    print(f"AIRP : {len(rows)} officines agréées ({sum(1 for r in rows if r['lat'])} avec GPS)")

    out = RAW / "airp"
    out.mkdir(parents=True, exist_ok=True)
    for name, path in (("amm_decisions", "amm/decisions"), ("qualite_alertes", "mqif/alerts")):
        p = f.get(f"{API}/{path}", max_age_s=24 * HOUR)
        (out / f"{name}.json").write_text(p.text, encoding="utf-8")
        body = json.loads(p.text).get("data")
        n = len(body.get("decisions", [])) if isinstance(body, dict) else len(body or [])
        print(f"AIRP {name} : {n} enregistrements")
