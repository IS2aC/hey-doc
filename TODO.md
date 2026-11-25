# Hey-Doc · Fiche TODO

Le premier chantier est l’identité de Hey-Doc. Elle doit se retrouver dans le chat web, le profil WhatsApp et la façon d’expliquer les résultats. Le pilote doit ensuite permettre à une personne d’estimer son ordonnance au comptant et de trouver une pharmacie de garde dans un secteur couvert.

## 1. Créer la marque et la charte graphique

- [ ] Rédiger le brief de marque sur une page. Préciser le public ivoirien, la promesse, les situations d’usage et les messages de confiance.
- [x] Fixer la graphie du nom **Hey-Doc**, écrire la signature et préparer une description courte pour le web et WhatsApp.
  *Signature proposée dans [branding/README.md](branding/README.md), à valider.*
- [ ] Concevoir deux pistes visuelles. Montrer pour chacune le logo, les couleurs, la typographie et un extrait du chat avec un résultat d’ordonnance.
- [ ] Choisir une piste et produire le logo principal, son symbole, les versions monochromes, le favicon et l’avatar WhatsApp. Exporter les fichiers sources et les formats web.
  *Fait : mascotte SVG, favicon, avatar WhatsApp. Reste : versions monochromes.*
- [ ] Créer la charte graphique avec la palette, les contrastes, les typographies, les espacements, les règles d’iconographie et les usages du logo.
  *Fait : palette, typographie, usages du logo. Reste : contrastes vérifiés, espacements, iconographie.*
- [x] Définir le ton en français simple et rédiger les messages sensibles. Couvrir l’accueil, la demande de photo, le prix manquant, la ligne à confirmer et la garde indisponible.
- [ ] Traduire la charte en variables de design et composants réutilisables pour le chat, les boutons, les cartes médicament et pharmacie, et les états de chargement ou d’erreur.
  *Commencé : variables CSS dans [branding/theme.css](branding/theme.css) et aperçu des deux parcours.*

**Livrable** Une identité validée, un kit d’actifs et une charte illustrée par le chat web sur mobile et le profil WhatsApp.

## 2. Maquetter les deux parcours

- [ ] Prototyper le parcours ordonnance de la prise de photo au total en FCFA, avec une alternative de saisie manuelle.
- [ ] Maquetter les cas où la photo est floue, plusieurs produits correspondent, le conditionnement manque ou seules certaines lignes sont chiffrées.
- [ ] Maquetter la recherche de garde par quartier, commune ou GPS et par jour et heure. Prévoir le résultat vide, le planning absent et l’appel de l’officine.
- [ ] Tester les maquettes avec quelques utilisateurs du secteur pilote. Vérifier qu’ils distinguent le total du sous-total et qu’ils savent quelle action prendre ensuite.

## 3. Collecter et publier les données

- [ ] Délimiter le secteur pilote et identifier les pharmacies partenaires et les sources de plannings.
- [x] Extraire le catalogue MUGEF-CI de septembre 2026 et conserver la liste de 2022 comme historique.
- [ ] Extraire les annexes CMU HTA et diabète de 2025 par OCR et relire les noms, dosages, conditionnements et tarifs.
  *Extraction faite (178 lignes). Reste la relecture : 40 lignes OCR peu sûres, 49 non rapprochées.*
- [ ] Recueillir les prix comptant par présentation et officine, avec date de relevé et source. Conserver séparément les prix publiés par la MUGEF-CI et les tarifs conventionnés CMU.
- [ ] Collecter les coordonnées des officines et les créneaux de garde. Vérifier les périodes à cheval sur minuit et convenir d’une fréquence de mise à jour.
  *Commencé : 1 136 officines AIRP, annuaire avec GPS, 195 gardes de la semaine, complément Google Maps à relire. Reste : téléphones et horaires, créneaux après minuit, fréquence.*
- [x] Définir le modèle PostgreSQL. Donner un identifiant interne à chaque présentation et conserver les codes propres à chaque source.
- [x] Créer un import Rust réexécutable. Valider un lot avant publication, relever les doublons et les lignes ambiguës, et conserver la provenance.
  *Fait en Python dans [pipeline/](pipeline/README.md), avec fichiers de relecture et provenance par ligne.*

## 4. Développer l’API de données Rust

- [x] Initialiser le service Axum, les migrations PostgreSQL et l’accès aux données avec SQLx.
- [x] Implémenter les cinq routes et documenter les réponses avec des exemples.

  | Fonction | Route |
  |---|---|
  | Recherche catalogue | `GET /v1/products/search` |
  | Prix par présentation | `GET /v1/presentations/{id}/prices` |
  | Couverture par régime | `GET /v1/presentations/{id}/coverage` |
  | Estimation comptant | `POST /v1/estimates` |
  | Pharmacies de garde | `GET /v1/duty-pharmacies` |

- [x] Tester une ordonnance saisie à la main, le calcul partiel et le cas sans aucun prix comptant.
- [x] Tester une garde publiée le dimanche soir, un créneau après minuit et un secteur sans planning vérifié.

## 5. Intégrer Reducto et le moteur de conversation

- [ ] Constituer un corpus d’ordonnances manuscrites anonymisées, relu par un pharmacien.
  *L’archivage masqué des ordonnances consenties est en place. Reste à collecter et faire relire.*
- [x] Intégrer **Reducto r-1 Parse** derrière un adaptateur HTTP. Gérer les formats d’image, les délais, les erreurs et le nombre de tentatives.
- [x] Transformer les blocs transcrits en lignes et rapprocher les noms, dosages et formes du catalogue. Chiffrer les présentations et quantités suffisamment précises.
- [ ] Mesurer la qualité de transcription, les erreurs d’identification, la latence et le coût par ordonnance exploitable. Évaluer Reducto Extract si les blocs sont difficiles à exploiter.
  *En cours, non commité : routes de test `reducto-text`, `similar-products` et `llm-structure` (Ollama), qui renvoie la consommation de tokens.*
- [ ] Coder les parcours de conversation dans Rust. Gérer les précisions, les lignes ambiguës, la saisie manuelle et l’enchaînement vers la recherche de garde.
- [ ] Rédiger et vérifier les réponses d’orientation vers un professionnel pour les demandes de diagnostic ou de changement de traitement et les messages signalant une urgence.

## 6. Construire le chat web et le bot WhatsApp

- [ ] Développer le chat responsive avec SvelteKit et la charte Hey-Doc. Vérifier les parcours au clavier et sur un écran mobile.
- [ ] Brancher l’envoi d’image et la recherche de garde sur le service de conversation commun.
- [ ] Configurer le profil Hey-Doc et un numéro dédié sur WhatsApp Business Cloud API.
- [ ] Intégrer le webhook Meta. Valider les événements, dédupliquer les messages et traiter les photos en arrière-plan.
- [ ] Adapter les réponses aux deux canaux et vérifier que les mêmes entrées donnent les mêmes montants et résultats de garde.

## 7. Mettre en service et suivre le pilote

- [ ] Vérifier les conditions de traitement des données par Meta et Reducto avant l’usage de vraies ordonnances. Fixer la durée de conservation des photos et tester leur suppression.
- [ ] Déployer l’application, la base et le stockage temporaire. Configurer HTTPS, les secrets et les sauvegardes, puis tester la restauration.
- [ ] Contrôler les estimations sur un échantillon de prix en officine et les gardes par rapport aux plannings. Nommer la personne chargée de corriger les écarts.
- [ ] Lancer auprès d’un groupe pilote et suivre la part des ordonnances chiffrables, les écarts de prix, les gardes exploitables, les délais et le coût par traitement.
- [ ] Corriger les problèmes rencontrés avant d’étendre le périmètre géographique.

## Après le pilote

- [ ] Ajouter d’autres assureurs quand leurs listes et règles de prise en charge sont disponibles. Calculer le reste à payer quand le contrat et le tarif applicables sont connus.
- [ ] Évaluer Hermes Agent si de nouveaux parcours justifient une orchestration plus souple. Comparer le coût et la qualité avec le moteur Rust.
- [ ] Évaluer un RAG documentaire si le chat s’étend à des questions générales de santé et qu’un corpus validé est disponible.

[Présentation de la solution et contrat de l’API](README.md)
