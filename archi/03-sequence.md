# Diagramme de séquence

Le scénario montre la lecture d’une ordonnance, puis une recherche de garde. Une demande de garde directe entre au dernier bloc.

```mermaid
sequenceDiagram
    actor Patient
    participant Web as Chat web
    participant Meta as WhatsApp Cloud API
    participant Rust as Backend conversation Rust
    participant Vision as Reducto r-1 Parse
    participant API as API de données Rust
    participant DB as PostgreSQL

    alt Web
        Patient->>Web: Envoie une photo
        Web->>Rust: Charge la photo et transmet la session
    else WhatsApp
        Patient->>Meta: Envoie une photo
        Meta->>Rust: Webhook signé avec identifiant du message et du média
        Rust->>DB: Enregistre la tâche et déduplique le message
        Rust-->>Meta: Accuse réception
        Rust->>Meta: Récupère le média en arrière-plan
        Meta-->>Rust: Photo
    end

    Rust->>Rust: Vérifie le fichier et la qualité exploitable
    alt Photo inexploitable
        Rust-->>Patient: Demande une nouvelle photo via le canal d’origine
    else Photo exploitable
        Rust->>Vision: Transmet la photo utile pour parsing
        Vision-->>Rust: Blocs et texte transcrits
        Rust->>Rust: Normalise les blocs et signale les champs incertains
        loop Pour chaque ligne exploitable
            Rust->>API: Recherche de présentations
            API->>DB: Recherche par libellé, dosage et forme
            DB-->>API: Candidats
            API-->>Rust: Présentations candidates
        end
        opt Ligne ambiguë
            Rust-->>Patient: Demande une précision via le canal d’origine
            Patient->>Rust: Précision via le canal ou ligne laissée à confirmer
        end
        opt Couverture demandée ou régime connu
            Rust->>API: Consulte la couverture des présentations identifiées
            API->>DB: Lit les listes de couverture
            DB-->>API: Références avec régime et édition
            API-->>Rust: Référencement par présentation
        end
        Rust->>API: Demande le chiffrage des lignes identifiées
        API->>DB: Lit les relevés comptant applicables
        DB-->>API: Montants datés
        API-->>Rust: Total, sous-total ou prix indisponible
        Rust-->>Patient: Affiche l’estimation via le canal d’origine
    end

    opt Demande de pharmacie de garde
        Patient->>Rust: Envoie secteur et créneau via le canal
        Rust->>API: Recherche les gardes pour cet instant
        API->>DB: Filtre les officines et périodes de garde
        DB-->>API: Résultats et état du planning
        API-->>Rust: Officines ou motif d’absence de résultat
        Rust-->>Patient: Affiche les coordonnées ou le motif via le canal
    end
```

Les interactions directes avec le patient après réception représentent les messages transmis par le chat web ou l’API Meta. Les lignes encore ambiguës restent visibles dans la réponse et hors du montant calculé.
