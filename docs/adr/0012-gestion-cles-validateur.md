# ADR 0012 — Gestion des clés validateur

- **Statut :** Proposé — 🟢 future
- **Date :** Août 2026
- **Portée :** Sécurité opérationnelle — séparation des clés de signature validateur.
- **Décideur :** VinX Labs.

---

## 1. Problème

La clé validateur est actuellement **« chaude »** dans le process du nœud : elle signe
blocs, co-signatures, et dérive la clé P2P (libp2p PeerId). Un compromis du process
nœud compromet donc la clé de consensus.

## 2. Options envisagées

- **Remote signer (KMS/HSM)** : le nœud délègue la signature à un service dédié (ex.
  Web3Signer, Dirk, ou implémentation custom). La clé n'est jamais en mémoire du nœud.
- **Clé P2P distincte** : une clé éphémère pour la couche réseau, distincte de la clé
  de consensus → moins de surface en cas de leak P2P.
- **Rotation de clé validateur** : mécanisme on-chain de mise à jour de la clé publique
  sans downtime (`RegisterNewKey`, période de transition).
- **Enclave SGX** : exécution de la signature dans une enclave → hardware isolation.

## 3. Direction

- Séparer **clé de consensus** (long-terme, froide) et **clé P2P** (éphémère, chaude).
- Interface `RemoteSigner` dans `vinx-node` — initialement implémentée in-process
  (backward-compat), puis extractable vers un daemon séparé.
- Rotation de clé on-chain via `GovernanceAction::UpdateValidatorKey` (à définir).

## 4. Critères de validation (à écrire avant Accepté)

- [ ] Un validateur peut déléguer la signature à un remote signer sans interruption.
- [ ] La clé P2P est distincte de la clé de consensus à la genèse.
- [ ] La rotation de clé ne cause pas de slash ni de perte de slot de leader.
- [ ] `cargo test --workspace` vert.

## 5. Dépendances

- ADR 0029 (comité VRF) : la clé ECVRF est également à gérer.
- ADR 0051 (primitives crypto) : base Ed25519.
- Phase 2 minimum (rotation de clé nécessite un set de validateurs stable).
