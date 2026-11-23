# API de données Hey-Doc

Service Rust (Axum, SQLx, PostgreSQL) qui expose le catalogue, les prix, la couverture, l’estimation au comptant et les pharmacies de garde.

## Lancer

```sh
docker compose up -d --wait postgres
cd api
DATABASE_URL=postgresql://hey_doc:hey_doc_local@127.0.0.1:5433/hey_doc cargo run
```

Au démarrage, le service applique les migrations de `../database/migrations`. Il écoute sur `127.0.0.1:8080`, modifiable avec `HEYDOC_API_ADDR`. `RUST_LOG` règle le niveau des journaux. `GET /health` vérifie l’accès à la base.

La documentation interactive (Swagger UI) est servie sur [`/docs`](http://127.0.0.1:8080/docs) : on peut y essayer chaque route. La spécification OpenAPI est sur `/docs/openapi.json`. Elle est générée depuis le code avec `utoipa`.

## Tester

```sh
DATABASE_URL=postgresql://hey_doc:hey_doc_local@127.0.0.1:5433/hey_doc cargo test
```

Chaque test crée sa propre base temporaire, applique les migrations et charge des données fictives. La base `hey_doc` n’est pas modifiée.

## Conventions

- Les montants sont des chaînes décimales exactes en FCFA (`"2600.00"`), la devise est `XOF`.
- Les instants sont rendus en UTC, qui est aussi l’heure locale d’Abidjan toute l’année.
- Une erreur renvoie `{"error": "<code>", "message": "<texte>"}` avec le statut 400, 404 ou 500.
- Dans les réponses, `source` donne l’éditeur, la référence et la date de publication de la donnée.

## Routes

### `GET /v1/products/search?q=amoxi&limit=20`

Recherche par libellé, DCI, dosage, forme et conditionnement. Les correspondances exactes passent d’abord, puis les noms proches (`amoxiciline` trouve `Amoxicilline`). `q` contient au moins 2 caractères, `limit` va de 1 à 50.

```json
{ "query": "amoxi", "results": [
  { "id": 1, "label": "Amoxicilline Démo", "ingredient": "Amoxicilline",
    "strength": "500 mg", "dosage_form": "Gélule", "pack": "Boîte de 12" } ] }
```

### `GET /v1/presentations/{id}/prices?kind=cash&sector=1`

`kind` vaut `cash` (par défaut), `mugefci_published` ou `cmu_contracted`. `sector` est facultatif. Les relevés les plus récents viennent en premier.

```json
{ "presentation_id": 1, "kind": "cash", "currency": "XOF", "prices": [
  { "id": 2, "amount_fcfa": "2600.00", "observed_on": "2026-11-20",
    "sector_id": 1, "sector_city": "Abidjan", "sector_district": "Cocody",
    "pharmacy_id": 2, "pharmacy_name": "Pharmacie Riviera",
    "source": { "publisher": "Relevé officine", "reference": "releve-2026-11", "published_on": null } } ] }
```

### `GET /v1/presentations/{id}/coverage?scheme=cmu`

`status` répond pour toutes les listes de l’édition la plus récente : `listed`, `not_listed` (la liste existe mais ne contient pas la présentation) ou `list_unavailable` (aucune liste chargée). `listings` détaille chaque édition. Sans `scheme`, toutes les listes sont consultées.

```json
{ "presentation_id": 3, "scheme": "cmu", "status": "listed", "listings": [
  { "listing_id": 1, "insurer": "CNAM", "scheme": "CMU", "listed": true,
    "source": { "publisher": "CNAM", "reference": "Décision HTA diabète 2025", "published_on": "2025-09-29" } } ] }
```

### `POST /v1/estimates`

```json
{ "sector_id": 1, "pharmacy_id": null, "lines": [
  { "presentation_id": 1, "quantity": 2 },
  { "presentation_id": 2, "quantity": null } ] }
```

`quantity` est un nombre de boîtes de 1 à 100. `null` signifie qu’elle reste à confirmer. Une estimation compte de 1 à 50 lignes. Pour chaque ligne, le service retient le dernier prix comptant relevé dans le secteur, ou dans l’officine si `pharmacy_id` est fourni. À date égale, il garde le prix le plus élevé pour ne pas sous-estimer le budget.

```json
{ "currency": "XOF",
  "scope": { "sector_id": 1, "city": "Abidjan", "district": "Cocody", "pharmacy_id": null, "pharmacy_name": null },
  "amount_kind": "subtotal", "amount_fcfa": "5200.00", "priced_lines": 1, "excluded_lines": 1,
  "lines": [
    { "index": 0, "presentation_id": 1, "presentation": { "id": 1, "label": "Amoxicilline Démo", "…": "…" },
      "quantity": 2, "unit_price_fcfa": "2600.00", "line_total_fcfa": "5200.00",
      "price": { "price_id": 2, "amount_fcfa": "2600.00", "observed_on": "2026-11-20",
                 "pharmacy_id": 2, "pharmacy_name": "Pharmacie Riviera", "source": { "…": "…" } },
      "exclusion": null },
    { "index": 1, "presentation_id": 2, "presentation": { "…": "…" }, "quantity": null,
      "unit_price_fcfa": "1500.00", "line_total_fcfa": null, "price": { "…": "…" },
      "exclusion": "quantity_unconfirmed" } ] }
```

`amount_kind` vaut `total` si toutes les lignes sont chiffrées, `subtotal` si une partie l’est seulement, `none` si aucune ne l’est. Dans ce dernier cas, `amount_fcfa` est `null`. Motifs d’exclusion : `unknown_presentation`, `quantity_unconfirmed`, `no_cash_price`.

### `GET /v1/duty-pharmacies?sector=1&at=2026-11-22T23:30`

`at` accepte une date RFC 3339 avec décalage ou une heure locale d’Abidjan sans décalage. Il vaut maintenant par défaut. Une garde couvre son début et exclut sa fin, elle peut traverser minuit.

```json
{ "sector": { "id": 1, "city": "Abidjan", "district": "Cocody" },
  "at": "2026-11-22T23:30:00Z", "status": "on_duty", "pharmacies": [
  { "pharmacy_id": 1, "name": "Pharmacie des Lagunes", "phone": "+225 00 00 00 00",
    "address": "Près du carrefour", "latitude": "5.359952", "longitude": "-3.986160",
    "duty_starts_at": "2026-11-22T19:00:00Z", "duty_ends_at": "2026-11-23T08:00:00Z",
    "source": { "publisher": "Planning de garde", "reference": "cocody-s47", "published_on": "2026-11-16" },
    "directions_url": "https://www.google.com/maps/dir/?api=1&destination=5.359952,-3.986160" } ] }
```

`status` vaut `on_duty`, `none_in_schedule` (un planning vérifié couvre l’instant sans garde correspondante) ou `no_schedule` (aucun planning vérifié pour ce secteur à cet instant).
