"""Shared helpers: paths, polite cached HTTP, CSV output and text normalization."""

from __future__ import annotations

import csv
import hashlib
import json
import re
import time
import unicodedata
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from urllib.parse import urlsplit

import httpx

ROOT = Path(__file__).resolve().parents[2]
DATA = ROOT / "data"
RAW = DATA / "raw"
CACHE = DATA / "cache"
STAGING = DATA / "staging"
OUT = DATA / "out"

USER_AGENT = "HeyDocPipeline/0.1 (public health data collection)"


def now_iso() -> str:
    return datetime.now(UTC).replace(microsecond=0).isoformat()


@dataclass
class Page:
    url: str
    text: str
    fetched_at: str
    from_cache: bool


class Fetcher:
    """HTTP client that caches every response on disk and spaces requests per host.

    A cached page is reused as-is unless `max_age_s` says it is stale, so an interrupted
    crawl resumes where it stopped and a re-run does not hit the site again.
    """

    def __init__(self, delay_s: float = 1.5, timeout_s: float = 30.0) -> None:
        self.delay_s = delay_s
        self._last: dict[str, float] = {}
        self._client = httpx.Client(
            headers={"User-Agent": USER_AGENT, "Accept-Language": "fr"},
            timeout=timeout_s,
            follow_redirects=True,
        )

    def _paths(self, url: str) -> tuple[Path, Path]:
        host = urlsplit(url).netloc
        key = hashlib.sha1(url.encode()).hexdigest()
        base = CACHE / "http" / host
        return base / f"{key}.body", base / f"{key}.json"

    def cached(self, url: str) -> Page | None:
        """The cached copy of a page, without any network access."""
        body_path, meta_path = self._paths(url)
        if body_path.exists() and meta_path.exists():
            meta = json.loads(meta_path.read_text())
            return Page(url, body_path.read_text(encoding="utf-8"), meta["fetched_at"], True)
        return None

    def get(self, url: str, max_age_s: float | None = None) -> Page:
        body_path, meta_path = self._paths(url)
        if body_path.exists() and meta_path.exists():
            meta = json.loads(meta_path.read_text())
            age = time.time() - meta["fetched_ts"]
            if max_age_s is None or age < max_age_s:
                return Page(url, body_path.read_text(encoding="utf-8"), meta["fetched_at"], True)

        host = urlsplit(url).netloc
        wait = self._last.get(host, 0) + self.delay_s - time.monotonic()
        if wait > 0:
            time.sleep(wait)

        last_error: Exception | None = None
        for attempt in range(4):
            try:
                resp = self._client.get(url)
                self._last[host] = time.monotonic()
                if resp.status_code in (429, 500, 502, 503, 504):
                    raise httpx.HTTPStatusError(f"HTTP {resp.status_code}", request=resp.request, response=resp)
                resp.raise_for_status()
                break
            except (httpx.TransportError, httpx.HTTPStatusError) as e:
                last_error = e
                if isinstance(e, httpx.HTTPStatusError) and e.response.status_code < 500 and e.response.status_code != 429:
                    raise
                time.sleep(self.delay_s * 2 ** (attempt + 1))
        else:
            raise RuntimeError(f"échec après 4 tentatives : {url}") from last_error

        body_path.parent.mkdir(parents=True, exist_ok=True)
        body_path.write_text(resp.text, encoding="utf-8")
        fetched_at = now_iso()
        meta_path.write_text(json.dumps({"url": url, "fetched_at": fetched_at, "fetched_ts": time.time()}))
        return Page(url, resp.text, fetched_at, False)


def write_csv(path: Path, rows: list[dict], fields: list[str]) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", newline="", encoding="utf-8") as f:
        w = csv.DictWriter(f, fieldnames=fields, extrasaction="raise")
        w.writeheader()
        w.writerows(rows)
    return path


def read_csv(path: Path) -> list[dict]:
    # utf-8-sig: some tools (google-maps-scraper-kit, Excel) start the file with a BOM.
    with path.open(newline="", encoding="utf-8-sig") as f:
        return list(csv.DictReader(f))


def strip_accents(s: str) -> str:
    return "".join(c for c in unicodedata.normalize("NFKD", s) if not unicodedata.combining(c))


def clean(s: str | None) -> str:
    return re.sub(r"\s+", " ", s or "").strip()


_NAME_NOISE = re.compile(
    r"\b(pharmacie|pharmacy|pharma|officine|grande|gde|gd|nouvelle|nlle|nvlle|nouv|la|le|les|de|du|des|d|l|et|st|ste|saint|sainte|sarl|sa|ex)\b"
)


def name_key(name: str) -> str:
    """Comparable form of an establishment name: no accents, case, punctuation or filler words."""
    s = strip_accents(name).lower()
    s = re.sub(r"[^a-z0-9]+", " ", s)
    s = _NAME_NOISE.sub(" ", s)
    return " ".join(s.split())


# Côte d'Ivoire switched to 10-digit numbers on 31 January 2021 (ARTCI plan). Old 8-digit
# numbers get a 2-digit prefix chosen from their first two digits.
_MOBILE_PREFIX = {
    **{p: "07" for p in "07 08 09 47 48 49 57 58 59 67 68 69 77 78 79 87 88 89 97 98".split()},
    **{p: "05" for p in "04 05 06 44 45 46 54 55 56 64 65 66 74 75 76 84 85 86 94 95 96".split()},
    **{p: "01" for p in "01 02 03 40 41 42 43 50 51 52 53 70 71 72 73".split()},
}


def norm_phone(raw: str | None) -> str | None:
    """Return a 10-digit Ivorian number, or None when the input cannot be read as one."""
    digits = re.sub(r"\D", "", raw or "")
    if digits.startswith("00225"):
        digits = digits[5:]
    elif digits.startswith("225") and len(digits) in (11, 13):
        digits = digits[3:]
    if len(digits) == 10:
        return digits
    if len(digits) == 8:
        head = digits[:2]
        if head[0] in "23":
            return "27" + digits
        if head in _MOBILE_PREFIX:
            return _MOBILE_PREFIX[head] + digits
    return None


def split_phones(raw: str | None) -> list[str]:
    """All numbers found in a free-text phone field, normalized and de-duplicated."""
    out: list[str] = []
    for chunk in re.split(r"[/;,]|\bou\b|\bet\b", raw or ""):
        p = norm_phone(chunk)
        if p and p not in out:
            out.append(p)
    return out


def parse_amount(raw: str | None) -> str | None:
    """'13 780', '8 540,56' or '1 065' -> '13780', '8540.56', '1065'."""
    s = re.sub(r"[\s  ]", "", raw or "").replace(",", ".")
    if not re.fullmatch(r"\d+(\.\d{1,2})?", s):
        return None
    return s


def norm_coord(raw, lo: float, hi: float) -> str | None:
    """'5,3741613' -> '5.374161'; None when empty, unreadable or outside [lo, hi]."""
    try:
        v = float(str(raw).strip().replace(",", "."))
    except (TypeError, ValueError):
        return None
    return f"{v:.6f}" if lo <= v <= hi and v != 0 else None
