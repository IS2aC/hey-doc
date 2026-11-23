"""CMU decision no. 0016 of 29/09/2025 (hypertension and diabetes), a scanned PDF read by OCR.

Each list row is anchored on its row number (left column), then matched with the label and
the conventioned tariff on the same line. Unnumbered lines in the label column are molecule
headings (INN, strength, form) and apply to the brands below them. Every row keeps its OCR
confidence and must be reviewed by a person before publication.
"""

from __future__ import annotations

import json
import re

import pdfplumber

from .common import CACHE, RAW, STAGING, clean, parse_amount, write_csv

PDF = RAW / "Liste_des_Médicaments_contre_HTA_et_DIABETE.pdf"
DPI = 200
REVIEW_SCORE = 0.97


def _ocr_pages() -> list[list[dict]]:
    cache = CACHE / "ocr" / PDF.stem
    cache.mkdir(parents=True, exist_ok=True)
    engine = None
    pages = []
    with pdfplumber.open(PDF) as pdf:
        for n, page in enumerate(pdf.pages, start=1):
            path = cache / f"p{n}_{DPI}dpi.json"
            if not path.exists():
                if engine is None:
                    import numpy as np
                    from rapidocr import RapidOCR

                    engine = RapidOCR()
                img = page.to_image(resolution=DPI).original.convert("RGB")
                r = engine(np.array(img))
                boxes = [
                    {"x0": float(min(p[0] for p in b)), "x1": float(max(p[0] for p in b)),
                     "y0": float(min(p[1] for p in b)), "y1": float(max(p[1] for p in b)),
                     "text": t, "score": float(s)}
                    for b, t, s in zip(r.boxes, r.txts, r.scores)
                ] if r.txts else []
                path.write_text(json.dumps({"width": img.width, "boxes": boxes}, ensure_ascii=False))
            pages.append(json.loads(path.read_text()))
    return pages


def run() -> None:
    pages = _ocr_pages()
    rows, decision_text = [], []
    list_name = None
    for pno, page in enumerate(pages, start=1):
        width = page["width"]
        boxes = sorted(page["boxes"], key=lambda b: ((b["y0"] + b["y1"]) / 2, b["x0"]))
        title = next((b["text"] for b in boxes if "LISTE DES MEDICAMENTS" in b["text"].upper()), None)
        if title is None and list_name is None:
            decision_text.append(f"--- page {pno}\n" + "\n".join(b["text"] for b in boxes))
            continue
        if title:
            list_name = clean(title)
        num_col = width * 0.17
        price_col = width * 0.70
        anchors = [(i, b) for i, b in enumerate(boxes) if b["x1"] < num_col and re.fullmatch(r"\d{1,3}", b["text"].strip())]
        anchor_ys = [(b["y0"] + b["y1"]) / 2 for _, b in anchors]
        anchor_ids = {i for i, _ in anchors}
        # The scan is slightly tilted: compare each label with its own row number, not a fixed x.
        if anchors:
            num_col = min(b["x1"] for _, b in anchors)
        heading = None
        events = []
        for i, b in enumerate(boxes):
            yc = (b["y0"] + b["y1"]) / 2
            if i not in anchor_ids and num_col <= b["x0"] < price_col and not any(abs(yc - ay) < 22 for ay in anchor_ys):
                if "LISTE DES" not in b["text"].upper():
                    events.append((yc, "heading", b))
        for i, b in anchors:
            yc = (b["y0"] + b["y1"]) / 2
            events.append((yc, "row", b))
        by_id = {id(b): i for i, b in enumerate(boxes)}
        for yc, kind, b in sorted(events, key=lambda e: e[0]):
            if kind == "heading":
                heading = b
                continue
            near = lambda c: min((x for x in boxes if c(x)), key=lambda x: abs((x["y0"] + x["y1"]) / 2 - yc), default=None)
            label = near(lambda x: by_id[id(x)] not in anchor_ids and x["x0"] > (b["x0"] + b["x1"]) / 2
                         and x["x0"] < price_col and abs((x["y0"] + x["y1"]) / 2 - yc) < 22)
            price = near(lambda x: x["x0"] >= price_col and abs((x["y0"] + x["y1"]) / 2 - yc) < 30)
            scores = [s["score"] for s in (b, label, price) if s]
            price_fcfa = parse_amount(price["text"]) if price else None
            problems = []
            if not label:
                problems.append("libellé manquant")
            if not price_fcfa:
                problems.append("tarif illisible")
            if min(scores) < REVIEW_SCORE:
                problems.append(f"confiance OCR {min(scores):.2f}")
            rows.append({
                "list": list_name,
                "row_number": b["text"].strip(),
                "molecule_heading": clean(heading["text"]) if heading else "",
                "label": clean(label["text"]) if label else "",
                "price_raw": price["text"] if price else "",
                "price_fcfa": price_fcfa,
                "ocr_min_score": round(min(scores), 3),
                "review": "; ".join(problems),
                "source_file": PDF.name,
                "page": pno,
            })
    write_csv(STAGING / "cmu_hta.csv", rows, list(rows[0]))
    (STAGING / "cmu_hta_decision.txt").write_text("\n".join(decision_text), encoding="utf-8")
    flagged = sum(1 for r in rows if r["review"])
    print(f"CMU HTA/diabète : {len(rows)} lignes, {flagged} à relire en priorité, texte de la décision : {len(decision_text)} pages")
