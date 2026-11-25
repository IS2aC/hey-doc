# Diagramme d’activités

Les deux activités peuvent démarrer de façon indépendante. Après une estimation, la personne peut enchaîner avec la garde.

```mermaid
flowchart TD
    start(["Message web ou WhatsApp"])
    route{"Quelle demande ?"}
    clarify["Demander de choisir un parcours"]
    urgent["Donner l’orientation et le contact vérifié"]
    photo{"Photo ou saisie texte ?"}
    quality{"Photo exploitable ?"}
    retake["Demander une nouvelle photo ou une saisie texte"]
    extract["Transcrire avec Reducto r-1 Parse et normaliser les blocs en lignes"]
    match["Chercher les présentations dans le catalogue"]
    ambiguous{"Une donnée nécessaire reste ambiguë ?"}
    confirm["Demander confirmation ou signaler la ligne au pharmacien"]
    quote["Chiffrer les lignes exploitables au prix comptant"]
    estimate["Afficher le total ou sous-total, les lignes en attente et la couverture documentée"]
    follow{"Chercher une garde ?"}
    whereWhen{"Secteur et créneau connus ?"}
    missing["Demander le secteur ou le créneau manquant"]
    search["Rechercher dans le planning vérifié"]
    found{"Officine de garde trouvée ?"}
    show["Afficher officine, téléphone, repère et plage de garde"]
    unavailable["Indiquer le résultat vide ou le planning indisponible"]
    done(["Réponse envoyée"])

    start --> route
    route -->|"Ordonnance"| photo
    route -->|"Garde"| whereWhen
    route -->|"Urgence signalée"| urgent --> done
    route -->|"Demande floue"| clarify --> route
    photo -->|"Photo"| quality
    photo -->|"Texte"| match
    quality -->|"Non"| retake --> photo
    quality -->|"Oui"| extract --> match
    match --> ambiguous
    ambiguous -->|"Oui"| confirm
    confirm -->|"Précision reçue"| match
    confirm -->|"Ligne à confirmer en officine"| quote
    ambiguous -->|"Non"| quote
    quote --> estimate --> follow
    follow -->|"Oui"| whereWhen
    follow -->|"Non"| done
    whereWhen -->|"Non"| missing --> whereWhen
    whereWhen -->|"Oui"| search --> found
    found -->|"Oui"| show --> done
    found -->|"Non"| unavailable --> done
```

Si aucune ligne n’a de prix comptant vérifié, la réponse indique que le montant est indisponible.
