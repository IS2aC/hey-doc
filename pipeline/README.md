# Pipeline de données Hey-Doc

Collecte les sources publiques sur les établissements de santé, les gardes, les médicaments, les prix et la couverture. Il les normalise, rapproche les doublons entre sources et charge le résultat dans PostgreSQL. Chaque ligne chargée garde la trace de sa source, de la page ou de la ligne d'origine et de la méthode de rapprochement.

## Lancer

```sh
cd pipeline
uv sync --extra ocr
uv run python -m heydoc_pipeline all
```

Les étapes peuvent être lancées une par une : `site`, `airp`, `mugef`, `cmu-hta`, `dedup`, `positions`, `load`, `gmaps-match` (hors `all`, voir plus bas). Avant `load`, la base doit avoir reçu les migrations : il suffit de démarrer l'API une fois (voir [api/README.md](../api/README.md)). `DATABASE_URL` désigne la base, par défaut la base locale de Docker Compose.

| Étape | Rôle | Durée |
|---|---|---|
| `site` | Gardes de la semaine, tables de prix et CMU, annuaire (positions GPS des 2 209 fiches) | quelques minutes |
| `site --only details` | Fiches détaillées de l'annuaire (téléphone, horaires). Reprend là où elle s'est arrêtée. | environ 3 h 30 la première fois |
| `airp` | Registre public des officines agréées | quelques secondes |
| `mugef` | Listes MUGEF-CI de septembre 2026 et novembre 2022 (PDF texte) | 25 s |
| `cmu-hta` | Décision CMU HTA et diabète du 29/09/2025 (PDF scanné, OCR mis en cache) | 40 s la première fois |
| `dedup` | Rapprochement des produits et des établissements | quelques secondes |
| `positions` | Contrôle des positions GPS des pharmacies, propositions de correction dans `review_positions.csv` | quelques secondes (1 min 20 si OpenStreetMap ne répond pas) |
| `load` | Rechargement complet des tables de référence, en une transaction. Applique les lignes acceptées de `review_gmaps.csv` et `review_positions.csv` s'ils existent. | 15 s |
| `gmaps-match` | Rapprochement des résultats Google Maps avec les pharmacies sans GPS ou sans adresse | quelques secondes |

Toutes les réponses HTTP sont mises en cache dans `data/cache/` et les requêtes vers un même site sont espacées de 1,5 s. Une collecte interrompue reprend sans redemander les pages déjà reçues. Pour rafraîchir les gardes chaque semaine, relancer `site --only duties`, puis `dedup` et `load` : la page des gardes est considérée périmée au bout de 6 heures.

```sh
uv run pytest
```

## Sources

| Source | Volume au 23/11/2026 | Ce qui est extrait |
|---|---|---|
| [Registre AIRP](https://sigir.airp.ci/portail-public) (`/api/public/officines`) | 1 136 officines agréées | Nom, adresse, région, ville, GPS, numéro et validité de l'autorisation. Pas de téléphone. |
| AIRP, décisions d'AMM et alertes qualité | 0 enregistrement aujourd'hui | Sauvegardées à chaque passage dans `data/raw/airp/` pour être exploitées dès qu'elles sont publiées. |
| Annuaire pharmacies-de-garde.ci | 2 209 fiches, 118 catégories | Nom, catégorie, localisation, adresse, GPS. Téléphone, horaires et description sur la fiche détaillée. |
| Gardes pharmacies-de-garde.ci | 195 gardes (Abidjan et intérieur) | Commune et secteur, officine, pharmacien, téléphone, semaine de garde. |
| Prix pharmacies-de-garde.ci | 3 870 lignes (déc. 2024) | Code, nom, groupe, prix. Mêmes codes que la liste MUGEF-CI 2026. |
| Liste CMU pharmacies-de-garde.ci | 737 lignes | Nom, prix, DCI, classe, forme. Copie enrichie de la liste de 2022. |
| MUGEF-CI, septembre 2026 | 5 754 lignes | Code, libellé, groupe, DCI, catégorie, générique ou spécialité, prix, régime (RCO, IVSPL). |
| MUGEF-CI, novembre 2022 | 720 lignes | Code, libellé, groupe, DCI, catégorie, prix, régime CMU. |
| Décision CMU n° 0016 du 29/09/2025 | 178 lignes (117 HTA, 61 diabète) | Molécule et dosage, spécialité, prix de la colonne « PRIX PUBLIC ». Texte de la décision dans `data/staging/cmu_hta_decision.txt`. |
| Google Maps, recherche par officine | 135 pharmacies recherchées | Nom, catégorie, adresse, téléphone, GPS, identifiant de lieu. Seulement pour compléter les champs vides. |
| [OpenStreetMap](https://www.openstreetmap.org/copyright) (Nominatim, Overpass) | 10 communes d'Abidjan, 1 062 pharmacies | Limites des communes, et pharmacies cartographiées quand le serveur Overpass répond. Données © les contributeurs d'OpenStreetMap, licence ODbL. Une requête par jeu de données, mise en cache. |

pharmacies-de-garde.ci est un site privé (« All rights reserved »). Son robots.txt autorise l'exploration, mais la réutilisation de ses données dans le produit doit être validée avec l'éditeur. Les sources officielles (AIRP, MUGEF-CI, CNAM) sont prioritaires quand elles couvrent le même besoin.

Les conditions d'utilisation de Google interdisent le scraping de Google Maps. Les résultats sont relus avant chargement et chaque valeur reprise garde sa provenance (source « Google Maps (recherche par officine) », drapeau `gmaps_enrichi`), pour pouvoir être retirée.

## Sorties

- `data/staging/` : une table CSV par source, avec la provenance de chaque ligne.
- `data/out/` : les entités consolidées (`establishments`, `presentations`, prix, couverture, gardes) et les liens vers chaque enregistrement source.
- `data/out/review_*.csv` : les cas que le pipeline n'a pas tranchés. Rien n'y est fusionné.

Dans PostgreSQL, `establishments` regroupe tous les établissements et `establishment_sources` garde chaque enregistrement source brut (JSON) avec sa méthode de rapprochement. `pharmacies` contient les officines utilisées par l'API, reliées à leur établissement.

## Compléter les pharmacies avec Google Maps

1. Lancer la recherche avec google-maps-scraper-kit, un mot-clé par pharmacie de `data/gmaps/targets.csv` (correspondance dans `keyword_map.csv`), et copier le CSV produit dans `data/gmaps/results.csv`.
2. `uv run python -m heydoc_pipeline gmaps-match` écrit `data/out/review_gmaps.csv` : meilleur candidat par pharmacie, niveau de confiance (haute, moyenne, faible, rejeté, introuvable) et colonne `accept`, préremplie à « oui » pour la confiance haute.
3. Relire le fichier et corriger `accept`.
4. `load` applique les lignes acceptées en retrouvant l'établissement par `ref`. Il ne complète que les champs vides (GPS, adresse) et ajoute le téléphone s'il manque, sans jamais remplacer une valeur. Une position à plus de 30 km de la médiane de la ville est ignorée. La ligne relue est gardée dans `establishment_sources` (méthode `gmaps_search`, clé = identifiant de lieu Google).

## Vérifier les positions des pharmacies

L'API trie les pharmacies par distance (`POST /v1/chat/location`) et donne l'itinéraire vers chacune. Une position fausse envoie quelqu'un au mauvais endroit, donc une position douteuse n'est pas utilisée.

**D'où viennent les erreurs.** L'annuaire pharmacies-de-garde.ci publie des positions souvent fausses : 23 fiches partagent le même point à Cocody, et la moitié des positions de l'annuaire sans plus code dans l'adresse tombent hors de la commune de l'adresse. Le rapprochement aggravait le problème : quand la position AIRP et celle de l'annuaire divergeaient de plus de 2 km, celle de l'annuaire était retenue. C'est le cas de la Pharmacie Schiba (Angré, placée au Plateau) et de la Pharmacie Locodjoro Sarl (Niangon, rattachée par erreur à une fiche de Locodjoro, à Attécoubé). Le registre AIRP contient aussi des points copiés d'une officine à l'autre, plus rarement.

**Contrôles.** Une position est douteuse quand :
- à Abidjan, elle tombe hors de la commune de la pharmacie ou de toute commune citée dans son adresse (limites OpenStreetMap, 300 m de tolérance) ; Anyama, Bingerville et Songon n'ont pas encore de limite et sont contrôlées comme les villes de l'intérieur ;
- ailleurs, elle est à plus de 15 km de la médiane des pharmacies de la ville ;
- elle est identique, au mètre près, à celle d'une autre pharmacie de nom différent. Un point qui n'apparaît qu'une fois dans le registre AIRP appartient à l'officine AIRP correspondante : seules les autres sont signalées.

**Relecture.**
1. `uv run python -m heydoc_pipeline positions` écrit `data/out/review_positions.csv` : une ligne par position douteuse, la source de la position actuelle, les problèmes, et la meilleure autre position connue qui passe les contrôles (AIRP, autre fiche de l'annuaire, OpenStreetMap) avec un niveau de confiance. `haute` : position AIRP contrôlée sur les limites de commune, ou confirmée par une deuxième source à moins de 300 m. La colonne `accept` reste vide : aucune correction n'est appliquée sans relecture, même en confiance haute.
2. Relire le fichier. `accept` = `oui` applique la position proposée, ou `corrected_lat`/`corrected_lng` s'ils sont remplis. `accept` = `garder` confirme la position actuelle. Vide : la position reste écartée. Une nouvelle exécution de `positions` garde les décisions saisies, tant que la pharmacie et sa position n'ont pas changé.
3. `load` applique les décisions, puis refait les contrôles. Une position encore douteuse reste sur l'établissement (`establishments`) avec le drapeau `position douteuse : …`, mais pas sur la pharmacie (`pharmacies.latitude` et `longitude` vides) : l'API ne la propose plus dans les pharmacies proches et ne donne pas d'itinéraire. Une position corrigée porte le drapeau `position corrigée après relecture (source), ancienne lat,lng` et la ligne relue est gardée dans `establishment_sources` (méthode `position_review`).

## Règles de rapprochement

**Établissements.** Le registre AIRP sert de référence pour les officines. Une fiche de l'annuaire y est rattachée si son nom est proche dans la même ville, ou si elle est très proche en GPS avec un nom voisin. Le site publie souvent la même officine plusieurs fois : toutes ses fiches rejoignent le même établissement. Un point AIRP à plus de 30 km de la médiane de sa ville est signalé. Quand les deux positions divergent de plus de 2 km, la position de l'annuaire est retenue et l'écart est noté ; l'étape `positions` contrôle ensuite le choix. Une garde est rattachée par téléphone, sinon par nom dans la même ville en départageant par commune. Les noms sont comparés sans accents, ponctuation ni mots génériques (pharmacie, grande, nouvelle, de la…). Les numéros à 8 chiffres sont convertis au format à 10 chiffres.

**Produits.** La liste MUGEF-CI 2026 sert de référence. Les prix du site sont rattachés par code. Les éditions écrites autrement (2022, liste CMU du site, décision CMU) sont rapprochées par marque et nombres du libellé (dosages et conditionnement), puis départagées par DCI ou par les autres mots. Les lettres O lues à la place de zéros par l'OCR sont corrigées pour la comparaison seulement.

**Gardes.** La page indique « de 20h à 8h et les week-ends » pour la semaine du samedi au vendredi. Le chargement crée un créneau du samedi 8 h au lundi 8 h, puis un créneau de 20 h à 8 h chaque nuit du lundi au vendredi. Toutes les communes d'une ville couverte reçoivent un planning, pour distinguer « aucune officine de garde ici » de « pas de planning ».

## Résultat du 23/11/2026

- 2 479 établissements distincts, dont 1 602 pharmacies, 214 cliniques, 144 hôpitaux, 84 centres de santé et 77 laboratoires. 2 419 ont une position GPS. Téléphones et horaires viendront avec la collecte des fiches détaillées.
- 526 officines AIRP retrouvées dans l'annuaire, à travers 733 fiches.
- 147 des 195 gardes rattachées à un établissement connu. Les 48 autres sont créées et signalées.
- 6 142 présentations distinctes, 10 441 prix, 6 626 références de couverture.

| Fichier de relecture | Lignes | Contenu |
|---|---|---|
| `review_establishments.csv` | 418 | Pharmacies de l'annuaire absentes du registre AIRP : registre incomplet, officine fermée ou nom trop différent |
| | 76 | Fiches de l'annuaire proches de plusieurs officines AIRP |
| | 48 + 11 | Gardes sans établissement connu ou ambiguës |
| `review_products.csv` | 306 | Produits de 2022 absents de l'édition 2026 |
| | 49 | Lignes de la décision CMU non rapprochées, souvent à cause de l'OCR |
| `review_positions.csv` | 533 | Positions douteuses au 26/11/2026 : 252 propositions de confiance haute (247 AIRP, 5 annuaire confirmées par OpenStreetMap), 30 moyenne, 251 sans autre source. Tant qu'elles ne sont pas relues, les 533 positions sont écartées. |
| `review_gmaps.csv` | 135 | Pharmacies sans GPS ou sans adresse et leur meilleur candidat Google Maps : 11 de confiance haute, 54 moyenne, 62 faible, 8 introuvables |
| `data/staging/cmu_hta.csv` | 40 | Lignes OCR de confiance inférieure à 0,97, colonne `review` |

## Points à valider

- **Nature des prix.** Les montants MUGEF-CI sont chargés comme « prix publiés MUGEF-CI ». S'ils correspondent au prix public national fixé par l'État, ils pourraient servir au chiffrage au comptant. À confirmer avec un pharmacien avant de modifier l'estimation.
- **Codes sans légende.** Les régimes RCO et IVSPL, les catégories R, TPC, EP, EPF et les types G et S ne sont expliqués dans aucun document. G et S sont lus comme générique et spécialité.
- **Décision CMU.** La colonne s'intitule « PRIX PUBLIC » alors que la décision fixe des tarifs conventionnés. Les montants sont chargés comme tarifs conventionnés CMU. La liste ne couvre que l'HTA et le diabète : elle est chargée sous le régime « CMU (HTA et diabète) » pour ne pas être lue comme la liste CMU complète.
- **Taux de prise en charge.** Aucune source ne donne les taux MUGEF-CI ou CMU. Le barème CMU relève du décret n° 2017-195 du 22 mars 2017.
- **Horaires de garde.** Le début du samedi à 8 h est une hypothèse : la page ne précise pas l'heure de relève.
