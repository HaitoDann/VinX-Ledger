# ADR 0017 — Arrêt d'urgence & reprise

- **Statut :** Proposé — 🟢 future
- **Date :** Août 2026
- **Portée :** Exploitation — procédure sûre de suspension et reprise de la chaîne sur bug critique.
- **Décideur :** VinX Labs.

---

## 1. Problème

En cas de bug critique découvert en production (ex. inflation de supply, vecteur de DoS
consensus), il n'existe aucun mécanisme sûr pour **suspendre la chaîne** sans :
- Geler les soldes (interdit — violerait les droits des utilisateurs).
- Rollback unilatéral (centralisé, non trustless).
- Attendre la gouvernance normale (trop lent en urgence).

## 2. Options envisagées

| Option | Description | Compromis |
|---|---|---|
| A — Halt coordiné par gouvernance | K-of-M signent un `GovernanceAction::EmergencyHalt` | Trustless mais lent (K signatures nécessaires) |
| B — Halt unilatéral admin | L'admin clé signe un message hors-bande | Rapide mais centralisé |
| C — Halt automatique sur invariant | Le nœud panique sur violation d'invariant | Déjà en place (supply_invariant_holds) — ne couvre pas tous les cas |
| D — Procédure documentée seulement | Pas de mécanisme on-chain ; procédure de coordination off-chain | Pragmatique pour la phase actuelle |

## 3. Direction

- **Phase actuelle** : Option D + Option C (les gardes dures sur invariants paniquent déjà).
  Documenter la **procédure de coordination manuelle** (canal d'urgence, qui contacte qui,
  comment relancer après un snapshot finalisé).
- **Phase 2+** : Option A — `GovernanceAction::EmergencyHalt { resume_at: Option<Timestamp> }`
  avec seuil de gouvernance **abaissé** (ex. K=1 sur le comité d'urgence pré-désigné, mais
  jamais un singleton non-custodial).
- Le **gel de solde est interdit** (principe de la monnaie pure, ADR 0001). Seul la
  production de nouveaux blocs est suspendue.

## 4. Critères de validation (à écrire avant Accepté)

- [ ] Un `EmergencyHalt` signé par K validateurs suspend la production de blocs en < 2 blocs.
- [ ] La reprise (`resume_at`) relance la production sans rollback d'état.
- [ ] Aucun solde utilisateur n'est modifié par le halt.
- [ ] La procédure de coordination manuelle est documentée dans `GUIDE.md`.

## 5. Dépendances

- ADR 0032 (garde-fous de gouvernance) : le K-of-M d'urgence s'intègre au comité de gouvernance.
- ADR 0011 (décentralisation gouvernance) : base pour le seuil d'urgence.
