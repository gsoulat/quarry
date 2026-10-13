# Audit UX/UI — synthèse

Audit réalisé sur les captures du 13/10/2026 (thème sombre, iced 0.14). Ce document résume le système visuel retenu et l'ordre d'application ; les détails par écran sont dans l'historique du projet.

## Système visuel

| Jeton | Valeur | Usage |
|---|---|---|
| `BACKGROUND` | `#0f1219` | fond de fenêtre, gouttières |
| `SURFACE` | `#161b24` | panneaux |
| `SURFACE_SUNKEN` | `#11151c` | éditeur, corps de grille, champs |
| `SURFACE_RAISED` | `#1c2230` | en-têtes de grille, survol, popovers, dialogues |
| `BORDER` / `BORDER_STRONG` | `#252c3a` / `#343d4f` | séparateurs / boutons secondaires |
| `TEXT` / `TEXT_SECONDARY` / `MUTED` / `DISABLED` | `#e6eaf2` / `#b4bdcc` / `#8b95a7` / `#59627a` | texte |
| `PRIMARY` | `#7aa2f7` (+ hover `#8fb3ff`, pressed `#6a92e7`) | accent, sélection |
| `SUCCESS` / `WARNING` / `DANGER` | `#9ece6a` / `#e0af68` / `#f7768e` | états |

Overlays : survol blanc α 0,06 ; alternance de lignes blanc α 0,03 ; sélection PRIMARY α 0,14 (listes) et 0,18 (grilles) ; voile de dialogue noir α 0,55.

Typographie : `TITLE` 18 semibold, `BODY` 13, `SMALL` 12, `EYEBROW` 11 majuscules, `MONO` 13, `MONO_SMALL` 12. Grille de 4 px ; contrôles 28 px, barre d'outils 24 px, lignes de liste 24/40 px, grille 26 px (en-tête 28). Rayons : 4 (contrôles), 6 (onglets, popovers), 8 (dialogues), 0 (panneaux principaux).

Boutons : `primary` (un seul par zone), `secondary` (fond RAISED + bordure forte), `danger` ghost (plein seulement dans la confirmation finale), `ghost`. Désactivé = texte DISABLED, sans fond ni bordure.

## Ordre d'application

1. Aplatir la coque : plus de cartes flottantes, panneaux collés aux bords, gouttières de 6 px, filets sous la barre du haut et au-dessus de la barre d'état.
2. Onglet de requête actif sans bordure de champ (fond PRIMARY α 0,14).
3. Quatre styles de bouton dans `theme.rs`, remplacement des styles iced par défaut ; `Déconnecter` en ghost.
4. États désactivés lisibles.
5. Grilles : en-tête RAISED, alternance, cellules carrées, paddings.
6. Boutons d'export en vrais boutons `CSV / JSON / SQL / XLSX`.
7. Échelle typographique en constantes ; titres de table en BODY semibold TEXT.
8. Ligne de connexion : marqueur 8 px, un seul glyphe avant le nom, flèche à droite.
9. Ombres et voile plus clair sur dialogues et popovers.
10. États vides dans Données et Résultats.

À planifier ensuite (structure) : renommage d'onglet sur double-clic, infobulle conditionnelle, largeur de colonne au 90ᵉ percentile et alignement numérique, groupement de l'historique par jour, barres de coût dans le plan, niveau de détail du diagramme, police d'icônes.
