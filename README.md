# Hey-Doc

Hey-Doc aide les patients et leurs proches en Côte d’Ivoire à **prévoir le coût d’une ordonnance et à trouver une pharmacie de garde**. Le même assistant est accessible dans un chat web responsive et dans un bot WhatsApp.

## Mission et objectifs

- Aider une personne à prévoir son budget avant d’aller en pharmacie, même si elle a reçu une ordonnance manuscrite.
- Montrer pour chaque ligne ce qui a pu être identifié, le prix comptant relevé et ce qui reste à confirmer auprès d’un pharmacien.
- Indiquer si une présentation figure dans une liste CMU ou MUGEF-CI publiée. Ajouter d’autres assurances lorsque leurs données sont disponibles.
- Trouver une officine de garde pour un secteur et un créneau précis, avec un numéro de téléphone et un repère.

Ce document décrit l’architecture cible du pilote. Le premier livrable technique est l’API de données Rust.

## Parcours utilisateur

### Estimer une ordonnance sans bon d’assurance

La personne envoie une photo. Hey-Doc en extrait les lignes et recherche les présentations correspondantes dans le catalogue. Si la photo est floue, il demande une nouvelle photo. Si un nom, un dosage ou une quantité reste ambigu, il indique la ligne à faire confirmer. Une saisie texte permet aussi de rechercher un produit.

Pour chaque ligne chiffrable, la réponse donne le nom, le dosage, la forme, le conditionnement, le nombre de boîtes, le prix comptant unitaire en FCFA et le montant de la ligne. L’estimation précise le secteur ou l’officine de référence, la source et la date du relevé. Une ligne sans prix ou quantité confirmée reste visible. Le montant est alors un **sous-total des lignes chiffrées**. Si aucune ligne n’est chiffrable, aucun montant n’est affiché.

Une mention de couverture précise le régime et l’édition de la liste. Le reste à payer d’une personne dépend des règles de son régime, de ses droits et du tarif applicable. Le pilote affiche le référencement lorsque la présentation est vérifiée.

### Trouver une pharmacie de garde

La personne indique une commune, un quartier ou un point GPS, ainsi que le jour et l’heure souhaités. Le bot lui demande les éléments manquants. La réponse donne le nom de l’officine, son adresse ou son repère, son téléphone, la plage de garde publiée et, si disponible, un lien d’itinéraire. Elle invite à appeler pour vérifier le stock avant le déplacement.

Une garde peut traverser minuit. La recherche utilise l’heure locale `Africa/Abidjan` et la plage réelle du planning. La réponse indique si le planning vérifié ne contient aucune garde correspondante ou si le secteur n’est pas couvert par les données disponibles.

### Périmètre du chat

Le pilote couvre l’aide à la lecture, le chiffrage et l’orientation vers une officine. Une demande de diagnostic, de changement de traitement ou de posologie est redirigée vers un professionnel. Un message signalant une urgence reçoit une orientation immédiate à partir de contacts vérifiés. [Numéros utiles officiels](https://www.diplomatie.gouv.ci/index.php/numeros-utiles)

## Architecture retenue

**Un agent conversationnel à parcours contrôlés, piloté par Rust.** Le backend conserve l’état de la demande, choisit la prochaine étape, pose les questions manquantes et appelle les fonctions métier autorisées. Reducto r-1 Parse transcrit les photos en blocs structurés. Le backend normalise les blocs en lignes d’ordonnance. Un petit modèle texte peut aussi classifier une demande libre que les boutons, les règles ou l’état de session ne suffisent pas à router.

La recherche catalogue, la couverture, le calcul du montant et la garde reposent sur des données structurées. Un RAG documentaire classique serait utile si Hey-Doc élargit plus tard son périmètre à des questions générales avec des sources éditoriales validées. Pour les deux parcours du pilote, une recherche PostgreSQL par libellé, DCI et présentation suffit comme point de départ.

| Bloc | Stack proposée et rôle |
|---|---|
| Chat web | SvelteKit et TypeScript pour le chat responsive, l’envoi d’image et l’affichage des résultats. |
| Bot WhatsApp | Meta WhatsApp Business Cloud API. Axum valide le webhook, enregistre l’événement avec son identifiant pour dédupliquer les livraisons et accuse réception rapidement. Un traitement asynchrone récupère le média et envoie la réponse. |
| Conversation | Rust, Axum, Tokio et Serde pour router les messages, valider les sorties du modèle, gérer l’état de session et produire des réponses adaptées au canal. |
| API métier | Rust, Axum, SQLx et PostgreSQL. Les modules conversation et données peuvent être déployés dans le même service au pilote. Recherche par champs normalisés et similarité avec `pg_trgm`. PostGIS pourra servir au classement GPS si nécessaire. |
| Imports et traitements | Commande Rust pour valider et publier des fichiers CSV. Extraction texte pour les PDF numériques, OCR avec relecture pour les scans. Une table PostgreSQL sert de file persistante pour reprendre les traitements asynchrones. |
| Lecture d’ordonnance | Adaptateur HTTP pour Reducto r-1 Parse. Reducto renvoie des blocs et du texte. Rust normalise les lignes et rapproche les candidats du catalogue. |
| Hébergement | Conteneurs derrière HTTPS, PostgreSQL sauvegardé et stockage objet privé à durée limitée pour les images en cours de traitement. |

## Collecte et publications

| Source | Données à intégrer |
|---|---|
| PDF MUGEF-CI de septembre 2026 | Catalogue de départ, références RCO et IVSPL et prix publiés dans cette édition. |
| PDF de novembre 2022 | Références historiques MUGEF-CI marquées CMU, utiles pour rapprocher d’anciens libellés. |
| Décision CMU HTA et diabète du 29 septembre 2025 | Présentations des annexes et tarifs conventionnés. Extraction OCR et contrôle visuel avant publication. |
| Pharmacies partenaires | Prix comptant datés, rattachés à une présentation et à une officine ou un secteur. |
| Annuaire et plannings validés | Identité des officines, contacts, secteur et périodes de garde. [AIRP](https://sigir.airp.ci/portail-public) · [Direction de l’Activité Pharmaceutique](https://dap.ci/services-v3/) |

Le pipeline conserve le document ou relevé d’origine, son édition et la page de la ligne. Il normalise les champs, soumet les correspondances ambiguës à relecture et publie un lot validé. Un identifiant interne représente la présentation exacte. Les codes des sources restent du texte pour préserver les zéros initiaux. Les trois types de montants sont conservés séparément en décimal exact **prix comptant, prix publié par la MUGEF-CI et tarif conventionné CMU**.

## Contrat de l’API de données

| Route | Contrat du pilote |
|---|---|
| `GET /v1/products/search?q=…` | Candidats avec identifiant interne, libellé, DCI, dosage, forme et conditionnement. |
| `GET /v1/presentations/{id}/prices?kind=cash` | Montants du type demandé, en FCFA, avec source, date et zone ou officine si applicable. |
| `GET /v1/presentations/{id}/coverage?scheme=cmu` | Présence dans la liste consultée, régime, édition et source. L’API distingue une absence dans la liste d’une liste indisponible. |
| `POST /v1/estimates` | Reçoit les présentations, les quantités et le secteur de l’estimation. Calcule chaque ligne à partir d’un prix comptant vérifié selon le périmètre annoncé. Renvoie le total ou le sous-total, les références des prix et les motifs des exclusions. |
| `GET /v1/duty-pharmacies?sector=…&at=…` | Officines dont la garde couvre l’instant demandé, période, coordonnées et source du planning. |

L’API métier reçoit des identifiants de produits et les paramètres de recherche. Le traitement des photos et les identifiants de conversation restent dans le module conversation. Les médias passent par un stockage privé temporaire. Les journaux techniques excluent le contenu des ordonnances, les numéros de téléphone et la localisation précise. Avant un pilote avec de vraies ordonnances, il faut valider la politique de conservation et de suppression, ainsi que les conditions de traitement par Meta et le fournisseur du modèle.

## LLM et coûts

**Fournisseur principal pour les photos d’ordonnance** — Reducto r-1 via l’API Parse. Le modèle prend en charge les pages photographiées et le manuscrit. Parse renvoie des blocs et du texte. L’API Extract est un produit distinct qui extrait des champs selon un schéma JSON et inclut le parsing. On l’évaluera si la normalisation des blocs en Rust donne trop peu de lignes exploitables. [Reducto Parse](https://reducto.ai/parse) · [Reducto Extract](https://reducto.ai/extract)

| Option | Tarif standard indicatif | Scénario de 10 000 pages |
|---|---|---|
| Reducto r-1 Parse, choix de départ | 10 USD pour 1 000 pages | 100 USD |
| Reducto Extract, parsing compris | 20 USD pour 1 000 pages | 200 USD |
| Gemini 2.5 Flash-Lite, référence de comparaison | 0,10 USD par million de tokens en entrée texte/image et 0,40 USD en sortie | 3,20 USD dans le scénario de 2 000 tokens facturés en entrée et 300 en sortie par page |

Estimations aux tarifs consultés le 22 novembre 2026, avec une page par photo, hors relances, options payantes, hébergement, stockage, vérification humaine et frais WhatsApp. Le scénario Gemini illustre un coût d’inférence. La qualité sur les ordonnances manuscrites et les conditions de traitement des données restent à vérifier pour chaque fournisseur. [Tarifs Reducto](https://reducto.ai/pricing/) · [Tarifs Gemini](https://ai.google.dev/gemini-api/docs/pricing) · [Tarifs Meta](https://business.whatsapp.com/products/platform-pricing)

Les gardes, les calculs et les confirmations guidées passent par Rust sans appel à Reducto. Sur un corpus anonymisé relu par un pharmacien, on mesurera le coût par ordonnance exploitable, la latence, les erreurs de nom ou de dosage et la proportion de lignes ambiguës correctement signalées. En cas d’ambiguïté persistante, le chat demande une photo ciblée ou une confirmation en pharmacie. Avant d’envoyer une ordonnance réelle, l’équipe doit vérifier les conditions Reducto de conservation et de suppression qui s’appliquent à son offre. [Offres Reducto](https://reducto.ai/pricing/)

**Place de Hermes.** Il peut être essayé comme moteur conversationnel derrière le backend, en lui exposant uniquement des fonctions métier autorisées. Le backend garde la maîtrise des validations et des actions. L’adaptateur WhatsApp du pilote reste dans Axum. Un essai de la passerelle Hermes se ferait sur un environnement distinct. L’option sera retenue si elle améliore les parcours mesurés à coût et qualité acceptables.

## Ordre de réalisation

1. Choisir un secteur pilote et importer le catalogue 2026, les références CMU contrôlées, un échantillon de prix comptant et un planning local.
2. Vérifier les cinq routes Rust avec une ordonnance saisie à la main et une garde le dimanche soir. Tester aussi un prix manquant et un planning absent.
3. Évaluer Reducto r-1 Parse et comparer les alternatives sur un corpus anonymisé relu par un pharmacien et mesurer le coût par ordonnance exploitable.
4. Brancher le chat web et WhatsApp sur le même parcours. Vérifier que les deux canaux donnent les mêmes résultats, qu’un webhook répété ne crée qu’un traitement et qu’une ligne ambiguë ne gonfle pas le total.

## Diagrammes

- [Architecture fonctionnelle et stack web et WhatsApp](archi/01-architecture-fonctionnelle.md)
- [Diagramme d’activités](archi/02-activites.md)
- [Diagramme de séquence](archi/03-sequence.md)

## Fiche de réalisation

- [TODO de Hey-Doc, du branding au pilote](TODO.md)
