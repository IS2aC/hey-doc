# Identité Hey-Doc

Hey-Doc aide les patients et leurs proches en Côte d’Ivoire à estimer le coût de leur ordonnance et à trouver une pharmacie de garde.

**Signature proposée** · « Votre ordonnance, votre budget. »

## Mascotte et logo

La mascotte fournie est au centre de l’identité. Son bandana turquoise et ses contours bleu profond donnent les repères du chat et du profil WhatsApp. Elle accompagne l’accueil, la demande de photo et les réponses d’aide. Les estimations, alertes et coordonnées d’officines conservent une présentation sobre, lisible d’un coup d’œil.

Le logo horizontal associe le SVG fourni au nom **Hey-Doc** en bleu profond. Le symbole seul convient au favicon et aux petits formats. Prévoir un espace libre autour de la mascotte et garder les proportions du SVG. L’avatar carré utilise un fond menthe et une marge prévue pour un recadrage circulaire.

## Couleurs et typographie

| Usage | Valeur |
|---|---|
| Vert de marque et illustrations | `#00B894` |
| Bleu profond des titres et montants | `#0F2747` |
| Menthe des fonds et messages accueillants | `#D1FAE5` |
| Vert foncé des boutons avec texte blanc | `#006B57` |
| Bleu d’interaction et focus | `#2563EB` |
| Ambre pour la vérification | `#F59E0B` et texte `#754500` sur `#FFF5DD` |
| Rouge pour les erreurs | `#EF4444` et texte `#A51F29` sur `#FFF0F1` |
| Fond, cartes, séparateurs et texte secondaire | `#F8FAFC`, `#FFFFFF`, `#E5E7EB`, `#475569` |

Utiliser une sans serif arrondie, avec **Nunito** comme option lorsque la police est disponible et une police système en secours. Les titres sont gras, les montants utilisent des chiffres tabulaires et affichent l’unité **FCFA**. Les cartes utilisent des angles de 20 px. Les couleurs d’état sont toujours accompagnées de texte.

## Voix et composants

| Situation | Texte ou comportement |
|---|---|
| Accueil du chat | « Envoyez une photo de votre ordonnance pour estimer votre budget. » |
| Photo inexploitable | « Reprenez la photo en cadrant toute l’ordonnance, avec assez de lumière. » |
| Estimation partielle | Afficher « Sous-total », le nombre de lignes chiffrées, puis la source et la date des prix. |
| Ligne ambiguë | « Faites confirmer le nom ou le dosage par votre pharmacien. » |
| Garde vérifiée | Donner le nom de l’officine, le secteur, le créneau et son téléphone. |
| Planning absent | « Aucun planning vérifié pour ce secteur et ce créneau. » |

## Fichiers

- [Aperçu web interactif](apercu.html), à ouvrir dans un navigateur. Les deux parcours sont illustrés avec des données fictives.
- [Variables CSS](theme.css) et [styles de l’aperçu](apercu.css).
- [Mascotte SVG et favicon](heydoc-mascotte.svg) et [avatar WhatsApp](actifs/avatar-whatsapp.png).
- [Planche de couleurs fournie](heydoc-branding-color.png), [mise en scène de la marque](mascotte-heydoc-vu%20conviviale.png) et [référence de la mascotte](mascotte-heydoc-remove-bg.png). Le damier est intégré à ce dernier PNG. Le SVG sert aux intégrations transparentes.
