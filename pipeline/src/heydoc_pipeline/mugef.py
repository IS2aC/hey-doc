"""MUGEF-CI reimbursable drug lists (text PDFs): one row per presentation, with provenance.

Cells are cut with the table rules drawn in the PDF: long vertical lines give the columns,
full-width horizontal lines give the rows. A name wrapped over three lines stays in its cell.
"""

from __future__ import annotations

import re
from pathlib import Path

import pdfplumber

from .common import RAW, STAGING, clean, parse_amount, write_csv

EDITIONS = {
    "2026-09": "Liste-des-medicaments-remboursables-Edition-Septembre-2026-11092026.pdf",
    "2022-11": "CMU-Liste-des-medicaments-remboursables-Edition-Novembre-2022-10112022.pdf",
}
KNOWN_CATEGORIES = {"R", "TPC", "EP", "EPF"}
HEADER_KEYS = {
    "CODE": "code", "NOM": "label", "GROUPE": "therapeutic_group", "DCI": "ingredient",
    "CATEG": "category", "TYPE": "type", "PRIX": "price_raw", "REGIME": "scheme",
}


def _merge(values: list[float], tol: float = 2.0) -> list[float]:
    out: list[float] = []
    for v in sorted(values):
        if not out or v - out[-1] > tol:
            out.append(v)
    return out


def _grid(page) -> tuple[list[float], list[float]]:
    table_h = page.height * 0.4
    xs = _merge([l["x0"] for l in page.lines if abs(l["x0"] - l["x1"]) < 1 and abs(l["bottom"] - l["top"]) > table_h])
    left, right = (xs[0], xs[-1]) if xs else (0, page.width)
    ys = _merge([
        l["top"] for l in page.lines
        if abs(l["top"] - l["bottom"]) < 1 and l["x0"] <= left + 5 and l["x1"] >= right - 5
    ])
    return xs, ys


def _cells(words: list[dict], xs: list[float]) -> list[str]:
    cols: list[list[dict]] = [[] for _ in range(len(xs) - 1)]
    for w in words:
        mid = (w["x0"] + w["x1"]) / 2
        for i in range(len(xs) - 1):
            if xs[i] <= mid < xs[i + 1]:
                cols[i].append(w)
                break
    return [clean(" ".join(w["text"] for w in sorted(c, key=lambda w: (round(w["top"]), w["x0"])))) for c in cols]


def extract(path: Path, edition: str) -> tuple[list[dict], list[dict]]:
    rows, rejects = [], []
    columns: list[str] | None = None
    with pdfplumber.open(path) as pdf:
        for pno, page in enumerate(pdf.pages, start=1):
            xs, ys = _grid(page)
            words = page.extract_words(keep_blank_chars=False, use_text_flow=False)
            for top, bottom in zip(ys, ys[1:]):
                band = [w for w in words if top <= (w["top"] + w["bottom"]) / 2 < bottom]
                if not band:
                    continue
                cells = _cells(band, xs)
                if len(cells) > 1 and cells[0].startswith("CODE") and cells[1].startswith("NOM"):
                    columns = [next((v for k, v in HEADER_KEYS.items() if c.startswith(k)), f"col{i}") for i, c in enumerate(cells)]
                    continue
                if columns is None:
                    continue
                rec = dict(zip(columns, cells))
                if not re.fullmatch(r"\d{6,10}", rec.get("code", "")):
                    if any(cells):
                        rejects.append({"edition": edition, "page": pno, "cells": " | ".join(cells)})
                    continue
                rows.append({
                    "edition": edition,
                    "code": rec["code"],
                    "label": rec.get("label", ""),
                    "therapeutic_group": rec.get("therapeutic_group", ""),
                    "ingredient": rec.get("ingredient", ""),
                    "category": rec.get("category", ""),
                    "type": rec.get("type", ""),
                    "price_raw": rec.get("price_raw", ""),
                    "price_fcfa": parse_amount(rec.get("price_raw")),
                    "scheme": rec.get("scheme", ""),
                    "review": "" if rec.get("category") in KNOWN_CATEGORIES else "catégorie illisible ou débordement de texte dans le PDF",
                    "source_file": path.name,
                    "page": pno,
                })
    return rows, rejects


def run() -> None:
    all_rows, all_rejects = [], []
    for edition, name in EDITIONS.items():
        rows, rejects = extract(RAW / name, edition)
        missing_price = sum(1 for r in rows if not r["price_fcfa"])
        print(f"MUGEF-CI {edition} : {len(rows)} lignes, {missing_price} sans prix lisible, {len(rejects)} lignes rejetées")
        all_rows += rows
        all_rejects += rejects
    write_csv(STAGING / "mugef.csv", all_rows, list(all_rows[0]))
    write_csv(STAGING / "mugef_rejects.csv", all_rejects or [{"edition": "", "page": "", "cells": ""}], ["edition", "page", "cells"])
