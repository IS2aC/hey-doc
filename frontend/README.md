# Landing page Hey-Doc

Ouvrir [index.html](index.html) dans un navigateur. La page réutilise les actifs de [branding](../branding/README.md). Elle fonctionne aussi sans serveur local. Les deux exemples affichent des données fictives.

## Répartition du travail

| Chantier | État et suite |
|---|---|
| Identité et landing page | Charte et mascotte intégrées. Landing responsive et démonstration des deux parcours disponibles. |
| Collecte et API · Claude | Importer le catalogue et les listes de couverture, recueillir des prix comptant et des plannings de garde, puis exposer les cinq routes documentées. PostgreSQL est déjà configuré sous Docker. |
| Intégrations · après l’API | Brancher le chat web et la lecture Reducto r-1, puis essayer le bot WhatsApp avec Kapso et des ordonnances fictives. |
| Validation du pilote | Comparer les montants aux prix en officine, vérifier les gardes et les conditions de traitement des photos, puis ouvrir le service sur le secteur retenu. |

Les appels à l’action de la landing ouvrent la démonstration jusqu’à la disponibilité du chat et du bot.
