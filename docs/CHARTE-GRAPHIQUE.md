# Charte graphique VinX Ledger

Elle découle du logo (`assets/logo.svg`), émeraude sur noir, avec une mise en page sobre inspirée des explorateurs de blockchain : gris neutres, l'émeraude réservée aux actions, aux liens et aux graphiques.
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

## Police

**DM Sans** (licence SIL Open Font License, `assets/fonts/`), intégrée aux binaires et servie par le nœud et le portefeuille à `/fonts/dm-sans.woff2` : aucune dépendance à Internet. Poids 400 pour le texte, 500 pour les titres, 600 pour les grands chiffres. Les adresses, clés et hash restent en police à chasse fixe.

## Formes

- **Coins arrondis** : 12 px pour les cartes, 8 px pour les champs, boutons et panneaux internes, pastilles entièrement arrondies.
- **Titres de carte** : en minuscules, 14 px, sur un bandeau légèrement hachuré.
- **Pastilles d'état** : fond teinté, sans bordure (vert, orange, rouge).
- **Liens** : émeraude, soulignés seulement au survol.
- **Onglets** : soulignés en émeraude.
- **Bouton principal** : émeraude plein. Les autres boutons sont bordés.

## Logo

- Il est dessiné en `currentColor` : il prend la couleur du texte qui l'entoure, en général l'accent.
- L'icône d'onglet du navigateur est le logo en `#0DDC8E`.
- L'en-tête affiche « VinX **Ledger** », suivi du nom de l'interface (Nœud ou Cockpit).
- Ne pas déformer le logo, ne pas lui ajouter d'ombre, ne pas le poser sur un fond émeraude.
