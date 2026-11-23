# Pipeline de données Hey-Doc

Collecte les sources publiques sur les établissements de santé, les gardes, les médicaments, les prix et la couverture. Il les normalise, rapproche les doublons entre sources et charge le résultat dans PostgreSQL. Chaque ligne chargée garde la trace de sa source, de la page ou de la ligne d'origine et de la méthode de rapprochement.

## Lancer

```sh
cd pipeline
uv sync --extra ocr
uv run python -m heydoc_pipeline all
```

Les étapes peuvent être lancées une par une : `site`, `airp`, `mugef`, `cmu-hta`, `dedup`, `load`. Avant `load`, la base doit avoir reçu les migrations : il suffit de démarrer l'API une fois (voir [api/README.md](../api/README.md)). `DATABASE_URL` désigne la base, par défaut la base locale de Docker Compose.

| Étape | Rôle | Durée |
|---|---|---|
| `site` | Gardes de la semaine, tables de prix et CMU, annuaire (positions GPS des 2 209 fiches) | quelques minutes |
| `site --only details` | Fiches détaillées de l'annuaire (téléphone, horaires). Reprend là où elle s'est arrêtée. | environ 3 h 30 la première fois |
| `airp` | Registre public des officines agréées | quelques secondes |
| `mugef` | Listes MUGEF-CI de septembre 2026 et novembre 2022 (PDF texte) | 25 s |
| `cmu-hta` | Décision CMU HTA et diabète du 29/09/2025 (PDF scanné, OCR mis en cache) | 40 s la première fois |
| `dedup` | Rapprochement des produits et des établissements | quelques secondes |
| `load` | Rechargement complet des tables de référence, en une transaction | 15 s |

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

pharmacies-de-garde.ci est un site privé (« All rights reserved »). Son robots.txt autorise l'exploration, mais la réutilisation de ses données dans le produit doit être validée avec l'éditeur. Les sources officielles (AIRP, MUGEF-CI, CNAM) sont prioritaires quand elles couvrent le même besoin.

## Sorties

- `data/staging/` : une table CSV par source, avec la provenance de chaque ligne.
- `data/out/` : les entités consolidées (`establishments`, `presentations`, prix, couverture, gardes) et les liens vers chaque enregistrement source.
- `data/out/review_*.csv` : les cas que le pipeline n'a pas tranchés. Rien n'y est fusionné.

Dans PostgreSQL, `establishments` regroupe tous les établissements et `establishment_sources` garde chaque enregistrement source brut (JSON) avec sa méthode de rapprochement. `pharmacies` contient les officines utilisées par l'API, reliées à leur établissement.

## Règles de rapprochement

**Établissements.** Le registre AIRP sert de référence pour les officines. Une fiche de l'annuaire y est rattachée si son nom est proche dans la même ville, ou si elle est très proche en GPS avec un nom voisin. Le site publie souvent la même officine plusieurs fois : toutes ses fiches rejoignent le même établissement. Un point AIRP à plus de 30 km de la médiane de sa ville est signalé. Quand les deux positions divergent de plus de 2 km, la position de l'annuaire est retenue et l'écart est noté. Une garde est rattachée par téléphone, sinon par nom dans la même ville en départageant par commune. Les noms sont comparés sans accents, ponctuation ni mots génériques (pharmacie, grande, nouvelle, de la…). Les numéros à 8 chiffres sont convertis au format à 10 chiffres.

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
| `data/staging/cmu_hta.csv` | 40 | Lignes OCR de confiance inférieure à 0,97, colonne `review` |

## Points à valider

- **Nature des prix.** Les montants MUGEF-CI sont chargés comme « prix publiés MUGEF-CI ». S'ils correspondent au prix public national fixé par l'État, ils pourraient servir au chiffrage au comptant. À confirmer avec un pharmacien avant de modifier l'estimation.
- **Codes sans légende.** Les régimes RCO et IVSPL, les catégories R, TPC, EP, EPF et les types G et S ne sont expliqués dans aucun document. G et S sont lus comme générique et spécialité.
- **Décision CMU.** La colonne s'intitule « PRIX PUBLIC » alors que la décision fixe des tarifs conventionnés. Les montants sont chargés comme tarifs conventionnés CMU. La liste ne couvre que l'HTA et le diabète : elle est chargée sous le régime « CMU (HTA et diabète) » pour ne pas être lue comme la liste CMU complète.
- **Taux de prise en charge.** Aucune source ne donne les taux MUGEF-CI ou CMU. Le barème CMU relève du décret n° 2017-195 du 22 mars 2017.
- **Horaires de garde.** Le début du samedi à 8 h est une hypothèse : la page ne précise pas l'heure de relève.
