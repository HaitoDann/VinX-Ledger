# ADR 0087 — Émission exponentielle continue, demi-vie de 10 ans

Statut : accepté (avant le mainnet ; remplace la courbe de l'ADR 0040)

## Contexte

L'émission était découpée en tranches de 20 ans, avec un débit constant dans chaque tranche :
la récompense par bloc chutait de moitié d'un coup tous les 20 ans (≈ 9,5 VINX par bloc au
départ). Ces paliers créent des effets de seuil pour les validateurs, et 20 ans est long pour
amorcer la sécurité du réseau.

## Décision

- Courbe cumulée **E(t) = S · (1 − 2^(−t/T½))**, S = 1 Md VINX, soit un débit
  R(t) = S·λ·e^(−λt), λ = ln 2 / T½. Plus aucune tranche.
- **T½ = 10 ans** (`EMISSION_T_HALF_SECS = 315 360 000`) : 500 M en 10 ans, 750 M en 20 ans.
- ~26,4 VINX par bloc de 12 s au lancement, −6,7 % par an, sans saut.
- Calcul entier déterministe : 2^(−t/T½) = 2^(−k) · e^(−x), k demi-vies entières,
  x = ln 2 · (t mod T½)/T½, e^(−x) par série de Taylor en virgule fixe 10^18. Erreur de
  quelques atomes, très inférieure à la récompense d'un bloc ; continuité aux frontières.
- `ERA0_EMISSION_ATOMS` est supprimé. Répartition inchangée (20 % proposeur, 80 % pot d'époque).

## Conséquences

- Changement de consensus : un réseau existant doit repartir d'une genèse neuve (le testnet
  n'est pas encore officiel ; aucun impact mainnet).
- Plus d'émission au début : ~29 % de l'offre en 5 ans (contre 12,5 % auparavant).
