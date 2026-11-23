# PostgreSQL local

Le projet utilise l’image officielle PostgreSQL 17 avec Docker Compose. Le port `5433` est publié sur la boucle locale et les données persistent dans un volume nommé.

Depuis la racine du dépôt, démarrer la base et appliquer la migration.

```sh
docker compose up -d --wait postgres
docker compose run --rm migrate
```

La connexion locale par défaut est `postgresql://hey_doc:hey_doc_local@127.0.0.1:5433/hey_doc`. Pour ouvrir psql dans le conteneur, utiliser `docker compose exec postgres psql -U hey_doc -d hey_doc`. Les variables `HEYDOC_DB_PORT` et `HEYDOC_DB_PASSWORD` permettent de configurer le port et le mot de passe. Initialiser un nouveau volume avec le mot de passe voulu. Changer la variable après initialisation ne modifie pas le mot de passe stocké par PostgreSQL.

Le service `migrate` applique la migration initiale dans une transaction et enregistre son numéro. La relance saute une migration déjà appliquée. Arrêter les conteneurs avec `docker compose down` conserve le volume.

## Schéma

- **Catalogue** · présentations, libellés d’origine et codes par source. Un index trigramme facilite la recherche textuelle.
- **Prix et couverture** · montants exacts en FCFA avec date, provenance et type. Le comptant, le prix publié MUGEF-CI et le tarif conventionné CMU ont des types distincts. Les listes de couverture sont identifiées par organisme, régime et source.
- **Officines et gardes** · secteurs, coordonnées, plannings et créneaux. Les périodes incluent le début et excluent la fin. Un planning publié sans garde correspondante se distingue d’un secteur sans planning pour le créneau.

Ce schéma prépare les cinq routes documentées dans le [README du projet](../README.md). Les données des PDF et les relevés en officine seront chargés lors de l’étape de collecte.

## Vérification

Le contrôle charge des données fictives dans une transaction annulée en fin d’exécution. Il vérifie notamment le calcul au comptant, la conservation des zéros du code produit et une garde couvrant minuit.

```sh
docker compose exec -T postgres psql -X -v ON_ERROR_STOP=1 -U hey_doc -d hey_doc < database/checks/smoke.sql
```
