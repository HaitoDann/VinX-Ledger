# ADR 0014 — Standard light-client

- **Statut :** Proposé — 🟠 moyenne priorité
- **Date :** Août 2026
- **Portée :** Accessibilité — format de preuve standardisé pour les clients légers (navigateur, mobile, SDK).
- **Décideur :** VinX Labs.

---

## 1. Problème

Les preuves Merkle d'état existent dans `vinx-crypto` mais aucun **format standardisé**
n'est défini pour :
- La chaîne d'en-têtes de blocs (comment un light-client vérifie la finalité sans rejouer toute la chaîne).
- Les checkpoints de *weak subjectivity* (point de départ sûr pour un fast-sync).
- Le format de preuve de solde (état Merkle → balance proof).

Résultat : pas de SDK light-client, pas de navigation décentralisée des balances.

## 2. Options envisagées

- **En-têtes seuls (SPV-style)** : le client télécharge tous les en-têtes (~200 o × N blocs),
  vérifie les co-signatures BFT, déduit la finalité. Léger mais lent à syncer.
- **Checkpoints de weak subjectivity** : le client démarre d'un checkpoint signé récent
  (validé par la gouvernance ou un ensemble de validateurs), ne télécharge que les en-têtes depuis.
  Rapide, mais requiert confiance initiale dans le checkpoint.
- **Preuves de finalité agrégées (BLS)** : un seul agrégat BLS par bloc finalisé → 1 co-sig
  agrégée par bloc finalisé → preuve de finalité en O(1). Possible grâce à ADR 0046.

## 3. Direction

- Standardiser le **format de balance proof** (merkle_proof_for + en-tête finalisé + co-sig BLS).
  Ce format existe déjà dans le code (`vinx-crypto::merkle_proof_for`) — formaliser la spec.
- Définir le format de **checkpoint** (hash du WorldState + hauteur + co-sig agrégée BLS).
- Publier un **SDK TypeScript/WASM** qui vérifie ces preuves côté navigateur.
- Intégration avec ADR 0034 (DA Celestia) pour les preuves de données d'Appchain.

## 4. Critères de validation (à écrire avant Accepté)

- [ ] Un navigateur peut vérifier une balance proof sans nœud complet.
- [ ] La preuve de finalité tient dans < 1 KB.
- [ ] Le fast-sync depuis un checkpoint prend < 30 s à 10 000 blocs/s réseau.
- [ ] L'ADR 0034 DA commitment est vérifiable depuis le même SDK.

## 5. Dépendances

- ADR 0046 (BLS) : prérequis pour la co-sig agrégée de finalité.
- ADR 0029 (comité VRF) : la co-sig du comité est ce qu'on agrège.
- ADR 0034 (DA Celestia) : preuves de données Appchain.
- Phase 3 minimum.
