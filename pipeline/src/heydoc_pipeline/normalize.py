"""Parsing of drug labels and places, shared by deduplication and loading."""

from __future__ import annotations

import math
import re
from difflib import SequenceMatcher

from .common import name_key, strip_accents

ABIDJAN_COMMUNES = {
    "abobo": "Abobo", "adjame": "Adjamé", "anyama": "Anyama", "attecoube": "Attécoubé",
    "bingerville": "Bingerville", "cocody": "Cocody", "koumassi": "Koumassi", "marcory": "Marcory",
    "plateau": "Plateau", "port bouet": "Port-Bouët", "port-bouet": "Port-Bouët", "songon": "Songon",
    "treichville": "Treichville", "yopougon": "Yopougon",
    # Neighbourhoods used as duty zones or site locations, mapped to their commune.
    "riviera": "Cocody", "angre": "Cocody", "ii plateaux": "Cocody", "deux plateaux": "Cocody",
    "2 plateaux": "Cocody", "palmeraie": "Cocody", "akouedo": "Cocody", "williamsville": "Adjamé",
    "vridi": "Port-Bouët", "anoumambo": "Port-Bouët", "adjouffou": "Port-Bouët", "gonzague": "Port-Bouët",
    "abobodoume": "Attécoubé", "locodjoro": "Attécoubé", "zone 4": "Marcory", "zone 3": "Marcory",
    "biabou": "Abobo", "pk 18": "Abobo", "niangon": "Yopougon", "allokoi": "Abobo",
}


def plain(s: str | None) -> str:
    return re.sub(r"\s+", " ", re.sub(r"[^a-z0-9]+", " ", strip_accents(s or "").lower())).strip()


def commune_of(*texts: str | None) -> str | None:
    """First Abidjan commune named in the given texts, longest names first."""
    for text in texts:
        t = f" {plain(text)} "
        for key in sorted(ABIDJAN_COMMUNES, key=len, reverse=True):
            if f" {plain(key)} " in t:
                return ABIDJAN_COMMUNES[key]
    return None


def city_key(s: str | None) -> str:
    return plain(s).replace(" ", "-")


def distance_m(lat1, lng1, lat2, lng2) -> float | None:
    if None in (lat1, lng1, lat2, lng2) or "" in (lat1, lng1, lat2, lng2):
        return None
    p1, p2 = math.radians(float(lat1)), math.radians(float(lat2))
    dp, dl = p2 - p1, math.radians(float(lng2) - float(lng1))
    a = math.sin(dp / 2) ** 2 + math.cos(p1) * math.cos(p2) * math.sin(dl / 2) ** 2
    return 2 * 6371000 * math.asin(math.sqrt(a))


def name_similarity(a: str, b: str) -> float:
    ka, kb = name_key(a), name_key(b)
    if not ka or not kb:
        return 0.0
    if ka == kb:
        return 1.0
    ta, tb = set(ka.split()), set(kb.split())
    jaccard = len(ta & tb) / len(ta | tb)
    return max(jaccard, SequenceMatcher(None, ka, kb).ratio())


# --- Drug labels -----------------------------------------------------------------------

FORMS = [
    (r"\b(COMPRIMES?|COMP|CPR?S?|CP)\b", "Comprimé"),
    (r"\b(GELULES?|GELU|GEL)\b", "Gélule"),
    (r"\b(SIROP|SIR)\b", "Sirop"),
    (r"\b(SUSP(ENSION)?|SBV|P\.?SBV)\b", "Suspension"),
    (r"\b(SOL(UTION)?|SOLUT)\b", "Solution"),
    (r"\b(INJ(ECTABLE)?|AMP(OULES?)?)\b", "Injectable"),
    (r"\b(CREME|CR)\b", "Crème"),
    (r"\b(POMMADE|POMM?)\b", "Pommade"),
    (r"\b(COLLYRE|CY)\b", "Collyre"),
    (r"\b(SUPPO(SITOIRES?)?)\b", "Suppositoire"),
    (r"\b(SACH(ETS?)?|PDRE|POUDRE|GRAN(ULES)?)\b", "Poudre ou granulés"),
    (r"\b(OVULES?)\b", "Ovule"),
    (r"\b(GOUTTES?|GTTES?)\b", "Gouttes"),
    (r"\b(SPRAY|AEROSOL|INH)\b", "Inhalation ou spray"),
    (r"\b(GEL)\b", "Gel"),
]
_UNIT = r"(?:MG|G|MCG|µG|UG|ML|UI|%)"
_STRENGTH = re.compile(rf"(\d+(?:[.,]\d+)?)\s*({_UNIT})(?:\s*/\s*(\d+(?:[.,]\d+)?)\s*({_UNIT}))?")
_PACK = re.compile(r"\b(?:B|BT|BTE|BOITE|BOÎTE|PQ|PLAQUETTE|BLISTER|FL|FLACON|T|TUBE)\s*(?:/|DE)?\s*(\d+)\s*(ML|G|COMPRIMES?|SACHETS?|X\s*\d+\s*ML)?", re.I)


def label_features(label: str) -> dict:
    up = strip_accents(label).upper().replace(",", ".")
    strengths = [f"{m.group(1).replace(',', '.')}{m.group(2)}" + (f"/{m.group(3)}{m.group(4)}" if m.group(3) else "")
                 for m in _STRENGTH.finditer(up)]
    pack = None
    if m := _PACK.search(up):
        pack = m.group(0).strip()
    form = next((f for rx, f in FORMS if re.search(rx, up)), None)
    numbers = sorted({n.rstrip("0").rstrip(".") if "." in n else n for n in re.findall(r"\d+(?:\.\d+)?", up)})
    brand = re.split(r"[\s\-/]", re.sub(r"^[^A-Z0-9]+", "", up), maxsplit=1)[0]
    return {"strengths": strengths, "pack": pack, "form": form, "numbers": numbers, "brand": brand}


def ocr_digits(label: str) -> str:
    """Letter O read inside a number by OCR ('5OMG', '16O MG', 'C0MPRIME') is put back as a digit or letter."""
    s = re.sub(r"(?<=\d)[Oo]|[Oo](?=\d)", "0", label)
    return re.sub(r"(?<=[A-Z])0(?=[A-Z])", "O", s)


_GENERIC_WORDS = {"COMPRIME", "COMPRIMES", "COMP", "CP", "CPR", "BOITE", "DE", "B", "PELLICULE", "PELL", "SECABLE",
                  "GELULE", "MG", "ML", "G", "SOLUTION", "INJECTABLE", "ENROBE", "ENR", "A", "LIBERATION"}


def label_words(label: str) -> set[str]:
    up = strip_accents(ocr_digits(label)).upper()
    return {w for w in re.findall(r"[A-Z]{2,}", up) if w not in _GENERIC_WORDS}


def label_key(label: str) -> str:
    """Label reduced to brand + numbers, used to compare editions written differently."""
    f = label_features(ocr_digits(label))
    return f"{f['brand']}|{' '.join(f['numbers'])}"
