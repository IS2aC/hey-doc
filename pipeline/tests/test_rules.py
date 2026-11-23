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
