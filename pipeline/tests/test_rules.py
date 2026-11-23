from datetime import date, datetime, timezone

from heydoc_pipeline.common import norm_coord, norm_phone, parse_amount, split_phones
from heydoc_pipeline.load import duty_slots
from heydoc_pipeline.normalize import commune_of, label_key, name_similarity
from heydoc_pipeline.site_gde import parse_period


def test_phones_move_to_ten_digits():
    assert norm_phone("27 24 38 38 88") == "2724383888"
    assert norm_phone("24002875") == "2724002875"  # old landline
    assert norm_phone("+225 07 77 81 19 23") == "0777811923"
    assert norm_phone("07 08 09") is None
    assert split_phones("07 77 81 19 23 / 27 20 37 03 45") == ["0777811923", "2720370345"]


def test_amounts_and_coordinates():
    assert parse_amount("8 540,56") == "8540.56"
    assert parse_amount("1 065") == "1065"
    assert parse_amount("360.0") == "360.0"
    assert parse_amount("illisible") is None
    assert norm_coord("5,3741613", -90, 90) == "5.374161"
    assert norm_coord("0", -90, 90) is None


def test_duty_week_crosses_midnight_and_the_weekend():
    html = "<h2>Pharmacies de garde : du Samedi 21 Novembre au Vendredi 27 Novembre 2026</h2>"
    assert parse_period(html) == (date(2026, 11, 21), date(2026, 11, 27))
    slots = duty_slots(date(2026, 11, 21))
    utc = timezone.utc
    assert slots[0] == (datetime(2026, 11, 21, 8, tzinfo=utc), datetime(2026, 11, 23, 8, tzinfo=utc))
    assert slots[1] == (datetime(2026, 11, 23, 20, tzinfo=utc), datetime(2026, 11, 24, 8, tzinfo=utc))
    assert slots[-1][1] == datetime(2026, 11, 28, 8, tzinfo=utc)
    assert len(slots) == 6


def test_period_spanning_new_year():
    html = "du Samedi 27 Décembre au Vendredi 2 Janvier 2027"
    assert parse_period(html) == (date(2026, 12, 27), date(2027, 1, 2))


def test_labels_from_different_editions_share_a_key():
    assert label_key("AMEP 5 MG COMPRIME, BOÎTE DE 28") == label_key("AMEP CP 5MG B/28")
    assert label_key("METFORMIN-DENK 85O MG COMPRIME, BOITE DE 30") == label_key("METFORMIN-DENK CP 850MG B/30")
    assert label_key("AMEP CP 5MG B/28") != label_key("AMEP CP 10MG B/28")


def test_names_and_places():
    assert name_similarity("PHARMACIE DE LA ME (NOUVELLE)", "Nouvelle Pharmacie De La Mé") == 1.0
    assert name_similarity("Pharmacie Les Jasmins", "Pharmacie Fatima") < 0.6
    assert commune_of("ABOBO PK 18") == "Abobo"
    assert commune_of("ZONE AKOUEDO + PALMERAIE EXTENSION + ABATTA") == "Cocody"
    assert commune_of("Port-Bouët") == "Port-Bouët"


def test_gmaps_score_needs_a_pharmacy_with_matching_name_and_place():
    from heydoc_pipeline.gmaps import _score

    ours = {"name": "Pharmacie Amina T", "commune": "Abobo", "city": "Abidjan",
            "latitude": "", "longitude": "", "phones": "0707070707"}
    hit = {"title": "PHARMACIE AMINA T", "category": "Pharmacie", "address": "Route d'Alépé, Abobo, Abidjan",
           "latitude": "5.42", "longitude": "-4.02", "phone": ""}
    assert _score(ours, hit)[0] == "haute"
    assert _score(ours, {**hit, "category": "Supermarché"})[0] == "rejeté"
    assert _score(ours, {**hit, "title": "Pharmacie du Plateau", "address": "Plateau, Abidjan"})[0] == "faible"
    assert _score(ours, {**hit, "title": "Pharmacie X", "phone": "+225 07 07 07 07 07"})[0] == "haute"


def test_gmaps_fills_only_empty_fields():
    from heydoc_pipeline.load import apply_gmaps

    def est(ref, **kw):
        return {"est_id": ref, "city": "Daloa", "address": "", "lat": "", "lng": "", "phones": "", "flags": "", **kw}

    ests = [
        est("a", lat="6.870000", lng="-6.450000"),
        est("b", lat="6.880000", lng="-6.440000"),
        est("vide"),
        est("plein", address="Quartier Lobia", lat="6.875000", lng="-6.445000", phones="0707070707", flags="x"),
        est("loin"),
    ]
    google = {"accept": "oui", "gmaps_address": "Rue Google", "gmaps_phone": "+225 05 05 05 05 05",
              "gmaps_lat": "6.876", "gmaps_lng": "-6.446", "gmaps_place_id": "p1", "name_score": "0.95"}
    review = [
        {**google, "ref": "vide"},
        {**google, "ref": "plein", "gmaps_place_id": "p2", "accept": "OUI"},
        {**google, "ref": "loin", "gmaps_lat": "5.36", "gmaps_lng": "-4.01", "gmaps_place_id": "p3"},
        {**google, "ref": "a", "accept": ""},
    ]
    links, counts = apply_gmaps(ests, review)
    vide, plein, loin = ests[2], ests[3], ests[4]
    assert (vide["address"], vide["lat"], vide["lng"]) == ("Rue Google", "6.876000", "-6.446000")
    assert vide["phones"] == "0505050505" and vide["flags"] == "gmaps_enrichi"
    assert (plein["address"], plein["lat"], plein["lng"]) == ("Quartier Lobia", "6.875000", "-6.445000")
    assert plein["phones"] == "0707070707 0505050505" and plein["flags"] == "x ; gmaps_enrichi"
    assert loin["lat"] == "" and loin["address"] == ""  # Abidjan position for a Daloa pharmacy
    assert ests[0]["address"] == ""  # not accepted
    assert [l["record_key"] for l in links] == ["p1", "p2"]
    assert counts["gps"] == 1 and counts["adresse"] == 1 and counts["téléphone"] == 2
    assert counts["position à plus de 30 km de la médiane de la ville"] == 1
