from heydoc_pipeline.normalize import communes_named, plus_code
from heydoc_pipeline.positions import Checker, human_decision, in_polygon

SQUARE = [[(-4.05, 5.30), (-4.00, 5.30), (-4.00, 5.35), (-4.05, 5.35), (-4.05, 5.30)]]


def test_plus_codes_decode_full_and_short():
    assert plus_code("849VCWC8+R9", 0, 0) == (37.422062, -122.084063)  # Google's reference example
    # Short code completed near the reference: the directory listing of Pharmacie Schiba.
    assert plus_code("8XXG+6F4,Abidjan", 5.35, -4.0) == (5.348063, -4.023813)
    assert plus_code("Rue 12, Cocody", 5.35, -4.0) is None


def test_polygon_with_tolerance():
    assert in_polygon(5.32, -4.02, SQUARE)
    assert not in_polygon(5.352, -4.02, SQUARE)
    assert in_polygon(5.352, -4.02, SQUARE, tolerance_m=300)  # about 220 m outside


def _e(ref, name, lat, lng, commune="Plateau", address=""):
    return {"est_id": ref, "kind": "pharmacie", "name": name, "city": "Abidjan", "commune": commune,
            "address": address, "lat": lat, "lng": lng}


def test_checker_flags_wrong_commune_and_copies_but_not_the_airp_owner():
    owner = _e("E1", "Pharmacie Du Port", "5.320000", "-4.020000")
    copy = _e("E2", "Pharmacie Sainte Rita", "5.3200001", "-4.0200001")
    away = _e("E3", "Pharmacie Lointaine", "5.400000", "-4.020000")
    # The address names another mapped commune: either one is accepted.
    other = _e("E4", "Pharmacie Bis", "5.400000", "-3.950000", commune="Plateau", address="Rue 3, Cocody")
    cocody = [[(-3.99, 5.38), (-3.90, 5.38), (-3.90, 5.45), (-3.99, 5.45), (-3.99, 5.38)]]
    check = Checker([owner, copy, away, other], {"Plateau": [SQUARE], "Cocody": [cocody]},
                    airp={"E1": ("5.320000", "-4.020000")}, airp_count={("5.320000", "-4.020000"): 1})
    assert check.issues(owner) == []
    assert check.issues(copy) == ["même position que 1 autre(s) pharmacie(s)"]
    assert check.issues(away) == ["hors de Plateau (hors des communes de la carte)"]
    assert check.issues(other) == []


def test_only_reviewed_rows_are_kept_across_runs():
    assert not human_decision({"confidence": "haute", "accept": ""})
    assert human_decision({"confidence": "haute", "accept": "oui"})
    assert human_decision({"confidence": "aucune source", "accept": "garder"})
    assert human_decision({"confidence": "moyenne", "accept": "oui", "corrected_lat": "5.3"})


def test_every_commune_named_in_the_address_counts():
    assert communes_named("Plateau Dokoui - Cocody Cocody - 01 BP 6145") == ["Cocody", "Plateau"]
