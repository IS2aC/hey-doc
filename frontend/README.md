# Landing page Hey-Doc

Ouvrir [index.html](index.html) dans un navigateur. La page réutilise les actifs de [branding](../branding/README.md). Elle fonctionne aussi sans serveur local. Les deux exemples affichent des données fictives.

Le hero se joue au défilement comme une vidéo : la mascotte laisse place au téléphone, l’ordonnance est photographiée puis lue, et le sous-total s’affiche. La timeline est dans [landing.js](landing.js) (`playFilm`). Elle utilise GSAP et ScrollTrigger 3.13, copiés dans `vendor/` pour éviter un CDN. Si l’utilisateur demande moins d’animations, ou si GSAP ne se charge pas, la page affiche directement l’état final et les trois étapes en liste.

La section `#inscription` ([account.js](account.js)) propose la création de compte et la connexion, par numéro WhatsApp et mot de passe. Elle appelle `/v1/auth/*` avec le cookie de session et affiche directement le compte si une session est déjà ouverte. L’adresse de l’API est dans la balise `<meta name="heydoc-api">`. Le numéro du bot WhatsApp se renseigne dans `<meta name="heydoc-whatsapp">` : le bouton WhatsApp ouvre alors une conversation avec « Hey Doc » déjà écrit. Pour essayer en local, servez le dépôt sur le port 5173 (`python3 -m http.server 5173` à la racine) et lancez l’API.

## Répartition du travail

| Chantier | État et suite |
|---|---|
| Identité et landing page | Charte et mascotte intégrées. Landing responsive et démonstration des deux parcours disponibles. |
| Collecte et API · Claude | Importer le catalogue et les listes de couverture, recueillir des prix comptant et des plannings de garde, puis exposer les cinq routes documentées. PostgreSQL est déjà configuré sous Docker. |
| Intégrations · après l’API | Brancher le chat web et la lecture Reducto r-1, puis essayer le bot WhatsApp avec Kapso et des ordonnances fictives. |
| Validation du pilote | Comparer les montants aux prix en officine, vérifier les gardes et les conditions de traitement des photos, puis ouvrir le service sur le secteur retenu. |

Les appels à l’action de la landing ouvrent la démonstration jusqu’à la disponibilité du chat et du bot.
