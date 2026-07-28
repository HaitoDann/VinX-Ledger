# Décisions d'architecture (ADR) — VinX Ledger

Ce dossier trace les décisions d'architecture. Chaque ADR a un **statut** :
`Accepté`, `Proposé` (en attente de décision), `Rejeté`, `Remplacé`.

## Acceptés

| # | Titre | Statut |
|---|-------|--------|
| [0001](./0001-l1-monnaie-pure-modules-ancrage-bonde.md) | L1 monnaie pure + surcouches par ancrage bondé | Accepté (design, non implémenté) |

---

## Backlog proposé (améliorations de VinX)

Catalogue priorisé des ADR à écrire/décider. **Priorités :** 🔴 haute · 🟠 moyenne · 🟢 future.
Rien ci-dessous n'est décidé — ce sont des propositions à instruire une par une.

### Consensus & finalité

- **0002 — Finalité au quorum** 🔴
  *Problème :* aujourd'hui le producteur commite le bloc avec sa **seule** signature ; le quorum n'est vérifié qu'*a posteriori*. La « finalité déterministe immédiate » annoncée n'est donc pas garantie à n≥2.
  *Direction :* deux phases explicites — `propose → collecte quorum de co-signatures → commit`. Séparer un pointeur `finalized_height` du `tip`. Interdire toute réorg sous la hauteur finalisée.
  *Compromis :* latence de finalité = 1 aller-retour de co-signatures (au lieu d'un commit optimiste).

- **0005 — Temps réseau robuste** 🟠
  *Problème :* l'émission et la déliaison font confiance au `timestamp` du bloc, posé par un seul producteur. Bornes actuelles : monotonie + horloge locale à la production seulement.
  *Direction :* timestamp = médiane des horloges des validateurs (façon Bitcoin "median time past"), bornes strictes à la **réception** P2P (plafond futur, monotonie).
  *Compromis :* nécessite d'échanger/valider des horloges — pertinent surtout à n≥2.

### Sécurité

- **0003 — Slashing automatique de l'équivocation** 🔴
  *Problème :* la détection d'équivocation existe (`record_signature`) mais **rien ne construit ni ne diffuse** la transaction `SlashValidator`. La sanction repose sur une action manuelle.
  *Direction :* sur double-signature détectée → assembler l'`SlashEvidence` (les deux en-têtes signés), construire et gossiper une tx `SlashValidator` automatiquement.
  *Compromis :* gestion des faux-positifs (partitions réseau) — la preuve cryptographique les élimine, mais à cadrer.

- **0012 — Gestion des clés validateur** 🟢
  *Problème :* la clé validateur est « chaude » dans le process du nœud (elle signe blocs + co-signatures + dérive la clé P2P).
  *Direction :* séparation *signer* (remote signer / HSM), rotation de clé validateur, clé P2P distincte de la clé de consensus.

- **0016 — Posture post-quantique** 🟢
  *Direction :* documenter un chemin de migration (versioning d'adresse déjà possible via bech32, schéma de signature enfichable), sans l'implémenter maintenant. Ed25519 n'est pas PQ.

- **0017 — Arrêt d'urgence & reprise** 🟢
  *Problème :* aucun mécanisme sûr pour suspendre la chaîne sur bug critique (distinct du gel de compte, interdit par le protocole).
  *Direction :* halt coordonné par la gouvernance (pas de gel de solde), procédure de reprise/rollback documentée.

### Tokenomics & frais

- **0004 — Invariant exécutable** 🔴 *(quasi une tâche)*
  *Problème :* `circulation + Fonderie = 100 Md` est promis « à chaque bloc » mais n'est **jamais vérifié à l'exécution** (seulement en tests).
  *Direction :* `debug_assert!` (voire rejet de bloc) après `settle_block`. Ferme le risque de régression silencieuse le plus insidieux.

- **0009 — Frais des transactions stake/unstake** 🟠
  *Problème :* le whitepaper donne un poids `1` à stake/unstake, mais le code les **exempte** (fee ZERO). Incohérence + petit vecteur de spam.
  *Direction :* décider — soit facturer le forfait (aligne le whitepaper), soit assumer l'exemption et corriger le whitepaper. Traiter l'anti-spam.

- **0021 — Immutabilité de la courbe d'émission** 🟠
  *Problème :* le halving (8 ans) et le total sont des constantes ; leur statut (gouvernable ou gravé) n'est pas décidé.
  *Direction :* graver l'émission comme **immuable** (argument de confiance : personne, pas même l'admin, ne change la politique monétaire). À acter explicitement.

### Données, état & scaling

- **0013 — Cycle de vie de l'état** 🟢
  *Problème :* les comptes ne sont **jamais élagués** → croissance non bornée de l'état (la chaîne est prunée, pas l'état).
  *Direction :* expiration/rent d'état, nœuds d'archive, ou compaction des comptes dormants à solde nul.

- **0014 — Standard light-client** 🟠
  *Direction :* formaliser le format de preuve (état Merkle — déjà là — + chaîne d'en-têtes + preuve de finalité), et la *weak subjectivity* / checkpoints pour un fast-sync sûr.

- **0015 — Exécution parallèle** 🟢
  *Direction :* exécution parallèle des tx sans conflit d'accès — pertinent seulement à des dizaines de milliers de TPS soutenus. Risque de déterminisme à cadrer.

- **0020 — Sérialisation canonique consensus-critique** 🟠
  *Direction :* garantir l'encodage canonique (borsh/bincode) des structures qui entrent dans un hash signé, pour empêcher toute malléabilité inter-implémentations.

### Réseau P2P

- **0022 — Durcissement P2P / anti-DoS** 🟠
  *Direction :* limites de taille de message, quotas par pair, formalisation du scoring/ban (partiellement présent), robustesse du protocole de sync (checkpoints, back-pressure).

### Gouvernance

- **0007 — Unification des chemins de gouvernance** 🟠
  *Problème :* `AddValidator`/`RemoveValidator` existent en **type de tx** (0x05/0x06) **et** en `GovernanceAction` (via `AdminAction`), avec des sémantiques légèrement différentes.
  *Direction :* un seul chemin (probablement `AdminAction`), retirer les doublons.

- **0008 — chain_id par défaut sûr** 🟠
  *Problème :* tous les constructeurs et le `serde(default)` retombent sur `DEVNET` — piège anti-replay pour une tx désérialisée sans chain_id.
  *Direction :* pas de défaut silencieux (chain_id explicite requis), ou défaut le plus sûr.

- **0011 — Décentralisation de la gouvernance** 🟠
  *Direction :* clé admin unique → **signature à seuil / multisig** → éventuellement gouvernance par les validateurs. Roadmap graduelle, sans casser le modèle actuel.

### Modules (par-dessus l'ADR 0001)

- **0010 — Primitive d'ancrage & registre de modules** 🟠
  *Direction :* première tranche concrète de l'ADR 0001 — type de tx `AnchorState` (0x09), registre `ModuleEntry { operator, bond, anchor_head }`, actions `RegisterModule` / `DeregisterModule`. Réutilise bond + Merkle + payload générique.

- **0023 — Adjudication du slashing de module** 🟢
  *Direction :* comment une fraude d'opérateur de module est prouvée et sanctionnée (bond → réputation → preuves de fraude → zk), sans jamais exécuter la logique du module sur la L1.

### Robustesse & exploitation

- **0006 — Préavis d'upgrade en temps réel** 🟠
  *Direction :* migrer les préavis 7/30/90 j de la hauteur de bloc vers les **timestamps** (aligner sur émission + déliaison). Ferme l'écart doc↔code actuel.

- **0018 — Observabilité & SLO** 🟢
  *Direction :* formaliser métriques (déjà Prometheus/Grafana), alertes, objectifs de service, à mesure que le réseau grandit.

- **0019 — TLS natif (rustls)** 🟢
  *Direction :* HTTPS sur le RPC sans dépendre d'un reverse-proxy.

---

### Comment décider un ADR proposé

1. Copier le gabarit de l'ADR 0001 (Contexte / Décision / Modèle / Conséquences / Alternatives).
2. Instruire le compromis honnêtement (surtout : ce que ça coûte, pas seulement ce que ça apporte).
3. Passer le statut à `Accepté` (design) puis, à l'implémentation, référencer le commit/CHANGELOG.
