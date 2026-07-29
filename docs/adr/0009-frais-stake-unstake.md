# ADR 0009 — Frais des transactions stake / unstake

- **Statut :** Proposé
- **Catégorie :** Cohérence · **Priorité :** 🟠 moyenne
- **Date :** Juillet 2026

## Contexte

Le whitepaper attribue un **poids `1`** aux transactions `Stake` / `Unstake` (donc un
frais forfaitaire, comme un transfert). Le **code**, lui, les traite avec `fee = ZERO` :
`apply_stake` / `apply_unstake` ne prélèvent aucun frais.

C'est une **incohérence doc↔code**, et un petit **vecteur de spam** : un `Unstake` crée
une entrée dans `pending_unbonds` sans coût, et un `Stake`/`Unstake` répété ne coûte rien
(mitigé seulement par les limites par adresse du mempool et le bond minimum).

## Décision proposée

Trancher explicitement l'un des deux :

- **(A) Facturer le forfait** sur `Stake`/`Unstake` (poids 1, comme le transfert). Aligne
  le whitepaper, ajoute une symétrie anti-spam. Le frais va au producteur (comme les
  transferts).
- **(B) Assumer l'exemption** : poids `0` pour `Stake`/`Unstake`, et **corriger le
  whitepaper** en conséquence.

**Recommandation : (A)**, sauf si la simplicité prime — l'anti-spam symétrique est propre
et cohérent avec « toute tx utilisateur paie un forfait ».

## Conséquences

- **(A)** Le wallet doit joindre un frais et l'expéditeur doit avoir le solde pour ; les
  déliaisons/stakes deviennent non gratuits (bien).
- **(B)** Laisse une petite surface de spam, bornée par les limites par adresse + le bond.

Dans les deux cas : **doc et code réconciliés** (l'objectif premier de cet ADR).

## Alternatives écartées

- **Laisser l'incohérence** : rejeté.

## Notes d'implémentation

Option (A) : ajouter la vérification de frais dans `apply_stake`/`apply_unstake`
(débit + `block_fees`), et faire poser le frais par les clients. Option (B) : éditer le
whitepaper §4.1 (poids 0 pour stake/unstake).
