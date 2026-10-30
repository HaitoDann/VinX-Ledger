# Charte graphique VinX Ledger

Elle découle du logo (`assets/logo.svg`) : un V et un X anguleux, un losange et un triangle, émeraude sur noir.
Le tableau de bord du nœud et le cockpit partagent la même feuille de style, `assets/theme.css`, intégrée aux binaires à la compilation.

## Couleurs

| Rôle | Thème sombre (par défaut) | Thème clair |
|---|---|---|
| Fond | `#070a09`, dégradé émeraude en haut de page | clair, teinté d'émeraude en haut |
| Carte | `#121715` | blanc cassé |
| Accent émeraude | `#0ddc8e` (texte sur accent `#05140d`) | `#067a4e` |
| Avertissement | `#f5a524` | `#9a5b00` |
| Erreur | `#ff6b6b` | `#c0362c` |

Tous les couples texte/fond ont un contraste d'au moins 4,99:1 (WCAG AA).

Le thème sombre est celui par défaut. Le bouton rond de l'en-tête bascule en clair ; le choix est mémorisé dans le navigateur (`localStorage`, clé `vinx-theme`).

## Composants

- **Hero** (explorateur) : grand titre centré et barre de recherche arrondie.
- **Cartes de statistiques** (`.stat`) : icône, valeur principale, puis un panneau de quatre sous-indicateurs.
- **Titres de carte** : bandeau hachuré en tête de carte, lien « Voir plus › » à droite.
- **Graphiques** : aires lissées en dégradé émeraude, avec graduations.
- **Classements** : rang dans une pastille ronde, colonnes chiffrées alignées à droite.

## Formes

- **Angles** : rayons faibles (2 à 4 px). Les cartes et les tuiles ont le coin supérieur droit coupé (`clip-path`), comme les branches du logo.
- **Losange** : c'est le motif de la marque. Il précède les titres de section et sert de pastille d'état (en ligne, absent) et de repère dans les frises.
- **Titres de section** : en majuscules, espacés.
- **Onglets** : soulignés en émeraude.
- **Bouton principal** : émeraude plein. Les autres boutons sont bordés.

## Logo

- Il est dessiné en `currentColor` : il prend la couleur du texte qui l'entoure, en général l'accent.
- L'icône d'onglet du navigateur est le logo en `#0DDC8E`.
- L'en-tête affiche « VinX **Ledger** », suivi du nom de l'interface (Nœud ou Cockpit).
- Ne pas déformer le logo, ne pas lui ajouter d'ombre, ne pas le poser sur un fond émeraude.
