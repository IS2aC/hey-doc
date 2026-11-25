# API de données Hey-Doc

Service Rust (Axum, SQLx, PostgreSQL) qui expose le catalogue, les prix, la couverture, l’estimation au comptant, les pharmacies de garde et la lecture d’ordonnance.

## Lancer

```sh
docker compose up -d --wait postgres
cd api
DATABASE_URL=postgresql://hey_doc:hey_doc_local@127.0.0.1:5433/hey_doc cargo run
```

Au démarrage, le service applique les migrations de `../database/migrations`. Il écoute sur `127.0.0.1:8080`, modifiable avec `HEYDOC_API_ADDR`. `RUST_LOG` règle le niveau des journaux. `GET /health` vérifie l’accès à la base.

La lecture de photo d’ordonnance passe par Reducto. Elle s’active avec `REDUCTO_API_KEY` (clé créée sur [studio.reducto.ai](https://studio.reducto.ai)). `REDUCTO_MODEL` vaut `r-1` par défaut et `REDUCTO_BASE_URL` `https://platform.reducto.ai`. Sans clé, `POST /v1/prescriptions/read` répond 503 et la saisie texte reste disponible. Ne versionnez pas la clé : passez-la par l’environnement ou un fichier `.env` ignoré par git.

L’archivage des ordonnances, avec accord, utilise un stockage compatible S3. En local :

```sh
docker compose up -d --wait minio
S3_ENDPOINT=http://127.0.0.1:9000 S3_ACCESS_KEY=hey_doc S3_SECRET_KEY=hey_doc_local_minio cargo run
```

`S3_BUCKET` vaut `prescriptions` par défaut, `S3_REGION` `us-east-1` et `HEYDOC_ARCHIVE_DAYS` 365 jours. Sans `S3_ENDPOINT`, rien n’est archivé, même avec accord. La console MinIO est sur http://127.0.0.1:9001. MinIO ne publie plus d’image officielle : `compose.yaml` utilise la dernière image communautaire archivée, figée et sans mises à jour de sécurité, à réserver au développement. N’importe quel stockage compatible S3 la remplace sans changer le code.

La documentation interactive (Swagger UI) est servie sur [`/docs`](http://127.0.0.1:8080/docs) : on peut y essayer chaque route. La spécification OpenAPI est sur `/docs/openapi.json`. Elle est générée depuis le code avec `utoipa`.

## Tester

```sh
DATABASE_URL=postgresql://hey_doc:hey_doc_local@127.0.0.1:5433/hey_doc cargo test
```

Chaque test crée sa propre base temporaire, applique les migrations et charge des données fictives. La base `hey_doc` n’est pas modifiée. Les tests d’archivage utilisent un stockage en mémoire. Un test supplémentaire écrit dans un vrai MinIO :

```sh
S3_ENDPOINT=http://127.0.0.1:9000 S3_ACCESS_KEY=hey_doc S3_SECRET_KEY=hey_doc_local_minio cargo test -- --ignored
```

## Conventions

- Les montants sont des chaînes décimales exactes en FCFA (`"2600.00"`), la devise est `XOF`.
- Les instants sont rendus en UTC, qui est aussi l’heure locale d’Abidjan toute l’année.
- Une erreur renvoie `{"error": "<code>", "message": "<texte>"}` avec le statut 400, 404, 500, 502 (le lecteur d’ordonnance a échoué) ou 503 (lecteur non configuré).
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

### `POST /v1/prescriptions/read`

Formulaire `multipart/form-data` avec un champ `image` : JPEG, PNG ou HEIC de 10 Mo au plus, format reconnu par le contenu du fichier. Deux champs facultatifs : `consent` (`true` si la personne accepte de verser son ordonnance masquée au corpus d’annotation, `false` par défaut) et `channel` (`web` par défaut, `whatsapp` ou `api`). Le service envoie la photo à Reducto (`/upload`, puis `/parse` avec le modèle r-1), puis supprime le fichier et le résultat chez Reducto. Les erreurs temporaires (408, 409, 429, 5xx, délai dépassé) sont retentées deux fois. Hey-Doc ne conserve pas la photo, et les journaux ne contiennent pas le texte de l’ordonnance.

```sh
curl -F image=@ordonnance.jpg http://127.0.0.1:8080/v1/prescriptions/read
```

Les blocs transcrits deviennent des lignes. Les en-têtes et pieds de page, le médecin, le patient, les dates et les signatures sont écartés (`ignored_lines`). Une ligne qui commence par une posologie est rattachée au médicament précédent. Sur une ligne de médicament, la posologie est séparée du produit. La quantité n’est retenue que si un nombre de boîtes est écrit (`N°2`, `x2`, `2 boîtes`, `(1)`). Elle n’est jamais déduite de la posologie. `units` donne le nombre d’unités à délivrer quand il est écrit (`#21` : 21 comprimés) ; le parcours de conversation peut en déduire un nombre de boîtes une fois le conditionnement choisi. Un produit écrit sur deux lignes (`Amoxicillin + Clavulanic acid` puis `(Co-Amoxiclav) 500/125 mg/tab`) est regroupé et cherché sous ses deux noms. Les ordonnances en anglais sont reconnues (`Name of Patient`, `Sig.`, `every 8 hours`). Une ligne sans candidat ni dosage (nom du médecin, tampon, note) est écartée.

```json
{ "status": "read", "message": null, "ignored_lines": 2,
  "reader": { "provider": "reducto", "model": "r-1", "pages": 1 },
  "lines": [
    { "text": "Doliprane 1000 mg N°2", "instructions": ["1 cp 3 fois par jour"], "quantity": 2, "units": null,
      "low_confidence": false, "status": "matched",
      "candidates": [ { "id": 812, "label": "DOLIPRANE CP SEC 1000MG B/8", "ingredient": "PARACETAMOL",
                        "strength": "1000MG", "dosage_form": "Comprimé", "pack": "B/8", "score": 1.0 } ] } ] }
```

`status` vaut `read`, ou `unreadable` si la photo contient trop peu de texte ; `message` invite alors à reprendre la photo ou à saisir les médicaments. Pour chaque ligne, `status` vaut :

- `matched` : un candidat se détache ;
- `ambiguous` : plusieurs présentations proches, par exemple deux marques ou deux formes au même dosage, et la personne choisit ;
- `no_match` : rien de suffisamment proche dans le catalogue.

`archive_id` n’est rempli que si la personne a donné son accord et que l’archive est configurée. Voir « Archive d’annotation » plus bas.

`candidates` liste au plus 5 présentations. Le score combine la proximité du texte avec le libellé complet et avec le nom écrit, puis tient compte des dosages convertis en milligrammes (`1g` = `1000 mg`, `500/125 mg` = deux dosages) et de la forme écrite (`cp`, `tab`, `gélule`, `sirop`…). `low_confidence` signale une lecture incertaine selon Reducto. La ligne choisie et sa quantité se passent ensuite à `POST /v1/estimates`.

### Archive d’annotation

Le but est de constituer un corpus annoté pour entraîner plus tard un modèle local de lecture manuscrite. Rien n’est archivé sans `consent=true`, ni pour une photo HEIC.

Après la réponse, en arrière-plan :

1. Reducto a renvoyé la position de chaque ligne écrite (`return_ocr_data`). Chaque ligne est classée *médicament*, *posologie* ou *autre* : en-tête, patient, médecin, date, tampon, signature, ou tout ce qui n’est pas reconnu.
2. L’image archivée part d’un fond noir. Seules les lignes *médicament* et *posologie* y sont recopiées depuis la photo, après application de l’orientation EXIF. Un nom ou une signature que le lecteur n’a pas su lire reste donc masqué. Le réencodage en JPEG supprime toutes les métadonnées, dont la position GPS.
3. L’annotation (`heydoc.prescription-annotation/1`) donne la position de toutes les lignes, avec leur texte seulement pour les lignes conservées. Elle contient aussi les lignes rapprochées du catalogue et le statut de relecture `to_review`.
4. MinIO reçoit `reads/AAAA/MM/<id>/image.jpg` et `annotation.json`. La table `prescription_reads` les indexe : canal, modèle, nombre de zones conservées et masquées, texte conservé pour la recherche, statut de relecture.
5. La photo d’origine et la réponse brute de Reducto ne sont jamais écrites sur disque.

Une purge supprime les objets puis les lignes à expiration, au démarrage puis toutes les 6 heures. Sans OCR par ligne, un bloc reste visible seulement si toutes ses lignes sont conservées. Les annotations sont des pré-annotations : elles doivent être relues avant tout entraînement.

### `POST /v1/prescriptions/match`

Même traitement pour une ordonnance saisie au clavier, une ligne par médicament, 4 000 caractères au plus.

```json
{ "text": "Doliprane 1000 mg N°2\n1 cp 3 fois par jour\nAmoxicilline 500 mg x1" }
```

### Comptes : `POST /v1/auth/register`, `POST /v1/auth/login`, `POST /v1/auth/logout`, `GET /v1/auth/me`

Un compte s’identifie par son numéro WhatsApp. `register` reçoit prénom, nom, ville (1 à 80 caractères), `country` (code ISO à deux lettres), `whatsapp` (numéro international, normalisé au format E.164), `email` facultatif, `password` (8 à 128 caractères) et `consent`, qui doit valoir `true`.

```json
{ "first_name": "Awa", "last_name": "Koné", "country": "CI", "city": "Abidjan",
  "whatsapp": "+225 07 01 02 03 04", "email": null, "password": "un mot de passe long", "consent": true }
```

- `register` crée le compte et ouvre une session : 201 avec le compte. Un numéro qui a déjà un compte répond 409 `account_exists`. Une personne inscrite avant les comptes, sans mot de passe, complète son inscription.
- `login` reçoit `{"whatsapp", "password"}` et répond 200 avec le compte. Un numéro inconnu et un mauvais mot de passe donnent la même réponse 401 `invalid_credentials`, dans le même délai.
- `logout` ferme la session et efface le cookie : 204.
- `me` renvoie le compte de la session, ou 401 `not_authenticated`.

Les mots de passe sont hachés avec Argon2id. La session est un jeton aléatoire de 256 bits dans le cookie `heydoc_session` (HttpOnly, SameSite=Lax, 30 jours). La table `sessions` n’en garde que le SHA-256. En production, `HEYDOC_COOKIE_SECURE=1` ajoute l’attribut `Secure`. Un champ caché `website`, rempli seulement par les robots, fait ignorer la création sans erreur. La connexion n’a pas encore de limite de tentatives.

Les pages web servies sur une autre origine peuvent appeler l’API, avec le cookie de session. `HEYDOC_WEB_ORIGINS` liste ces origines, séparées par des virgules. Par défaut : `http://127.0.0.1:5173,http://localhost:5173`.

### Routes de test

Elles servent à explorer les ordonnances comme du texte. Elles ne rapprochent rien et n’archivent rien.

`POST /v1/test/reducto-text` prend le même formulaire que `/v1/prescriptions/read`. Elle renvoie tout le texte lu par Reducto (`text`), les lignes nettoyées (`lines`) et les blocs bruts avec leur type et leur confiance (`blocks`). Chaque appel est facturé par Reducto.

`POST /v1/test/similar-products` reçoit un texte, par exemple le `text` de la route précédente :

```json
{ "text": "Amoxicillin + Clavulanic acid\n(Co-Amoxiclav) 500/125 mg/tab #21\nParacetamol 500 mg/tab #5",
  "min_score": 0.35, "per_line": 5 }
```

Chaque ligne d’au moins 3 lettres est comparée à tous les libellés du catalogue par similarité cosinus sur les trigrammes de caractères, sans accents ni casse. `lines` donne les produits proches de chaque ligne, `products` la liste des produits cités ou proches sans doublon, avec le meilleur score et les lignes où ils apparaissent. `min_score` va de 0 à 1 (0,35 par défaut), `per_line` de 1 à 20 (5 par défaut).

```json
{ "catalogue_size": 6142, "method": "cosine_char_trigrams",
  "lines": [ { "index": 2, "text": "Paracetamol 500 mg/tab #5", "matches": [
    { "id": 4175, "label": "PARACETAMOL CP 500 MG B/10", "ingredient": "PARACETAMOL", "score": 0.781 } ] } ],
  "products": [ { "id": 4175, "label": "PARACETAMOL CP 500 MG B/10", "ingredient": "PARACETAMOL", "score": 0.781, "lines": [2] } ] }
```

Sous 0,45 environ, les correspondances sont souvent du bruit : « Lic. No. 12 » ressemble à « NO-SPALGIN » à 0,40.

`POST /v1/test/llm-structure` envoie un texte d’ordonnance à un modèle de langage, qui renvoie les médicaments en JSON. Le prompt système est dans [prompts/prescription-structure.md](prompts/prescription-structure.md) : copie exacte du passage, aucun nom de personne, pas de posologie, pas de correction d’orthographe. La réponse est contrainte par un schéma JSON et la température est à 0.

Le modèle passe par l’API d’Ollama, qui sert aussi bien Ollama Cloud qu’un Ollama local :

| Variable | Ollama Cloud | Ollama local |
|---|---|---|
| `LLM_MODEL` | nom d’un modèle cloud | nom d’un modèle téléchargé |
| `LLM_BASE_URL` | `https://ollama.com` (par défaut) | `http://localhost:11434` |
| `LLM_API_KEY` | clé créée sur ollama.com | vide |

Pour chaque médicament, le modèle donne aussi la DCI, même quand le nom écrit est déjà la molécule, et une classe thérapeutique (`therapeutic_class`). `matches` propose ensuite les `per_item` présentations du catalogue les plus proches (3 par défaut, de 0 à 10). Le score compte pour 50 % la molécule ou le nom, 25 % le dosage, 15 % la forme et 10 % la marque écrite dans le libellé. `strength_match` et `form_match` indiquent si le dosage et la forme écrits correspondent, et `therapeutic_group` donne la classe enregistrée dans le catalogue.

Sans `LLM_MODEL`, la route répond 503 `llm_unavailable`.

```json
{ "text": "Name of Patient\nAmoxicillin + Clavulanic acid\n(Co-Amoxiclav) 500/125 mg/tab\n#21\nParacetamol 500 mg/tab #5", "per_item": 3 }
```

La réponse donne la consommation (`usage` : modèle, tokens d’entrée et de sortie, durée) pour estimer le coût par ordonnance, puis chaque médicament avec ses champs (`source_text`, `name`, `ingredients`, `strengths`, `form`, `boxes`, `units`). `grounded` vaut `false` si le passage cité ne figure pas dans le texte envoyé : c’est une invention du modèle, à écarter. `matches` donne les produits du catalogue les plus proches, par similarité cosinus du nom et des dosages avec le libellé, et des molécules avec la DCI. `ignored` liste les lignes écartées, sans nom de personne.

Avant d’envoyer de vraies ordonnances à un modèle hébergé, vérifier la politique de conservation des données du fournisseur.
