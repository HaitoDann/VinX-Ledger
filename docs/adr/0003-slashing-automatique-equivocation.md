# ADR 0003 — Slashing automatique de l'équivocation

- **Statut :** Vérifié — ✅ implémenté (detection + auto-report)
- **Date :** Juillet 2026 · Implémenté août 2026
- **Portée :** Sûreté BFT — punition on-chain de la double-proposition.
- **Décideur :** VinX Labs.

---

## 1. Contexte

Un proposeur malhonnête peut tenter de **finaliser deux branches concurrentes** en signant
deux blocs différents à la même hauteur (équivocation / double-proposition). Sans punition
automatique, le risque économique est nul — l'attaquant n'a rien à perdre.

## 2. Options envisagées

| Option | Description | Compromis |
|---|---|---|
| A — Rapport manuel | Un opérateur humain détecte et soumet la preuve | Réaction lente, non automatique |
| B — Slash automatique ✅ | Le nœud qui reçoit un second bloc conflictuel assemble et soumet la preuve automatiquement | Réaction immédiate, pas de faux positifs (vérification crypto) |
| C — Pas de slashing | Réputation seulement | Non-trustless ; pas de dissuasion économique |

## 3. Décision

**Option B — Slashing automatique.**

Sur réception d'un second bloc **différent** du même proposeur à une hauteur **déjà scellée**,
le nœud :
1. Assemble une `SlashEvidence` contenant les deux en-têtes signés.
2. Construit une transaction `SlashValidator` (tx 0x07).
3. La soumet au mempool et la gossipe au réseau.

La preuve est **cryptographiquement vérifiée** avant application → pas de faux positifs.
Le rapporteur reçoit 10 % du bond slashé ; 90 % vont au `epoch_dist_emission_pot` (redistribués
aux validateurs honnêtes, ADR 0040).

## 4. Critères de validation

- [x] Double-proposition détectée et slashée automatiquement en test unitaire.
- [x] La `SlashEvidence` est vérifiée cryptographiquement avant tout slash.
- [x] 10 % → rapporteur, 90 % → `epoch_dist_emission_pot` (invariant de supply maintenu).
- [x] L'attaquant slashé est retiré du set actif.

## 5. Reste à implémenter

- **Tranche 2** : détection de l'équivocation via **co-signatures conflictuelles** (deux
  co-signatures du même validateur sur deux en-têtes différents à la même hauteur).
  Nécessite de conserver un buffer des deux en-têtes co-signés.
  → Voir ADR 0030 (Accountability des co-signatures conflictuelles).
