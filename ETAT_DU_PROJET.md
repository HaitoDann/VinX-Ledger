# VinX Ledger — État du Projet

> Document de référence interne — mis à jour à chaque sprint.
> Dernière mise à jour : août 2026 (**consensus multi-validateur éprouvé : banc n=3 réel, finalité au quorum, jailing/rotation, fork-choice câblé de bout en bout** + **ADR 0040 implémenté** : émission progressive sans La Fonderie, T_half ~20 ans — whitepaper v5.0). **Cadence de consensus révisée** (12 s fixe, 3 000 tx/bloc max — anti-fork).

---

> ## 🧭 Où en est le projet (lis ceci en premier)
>
> **Le socle L1 est solide** (banc n=3 éprouvé, 309 tests verts, clippy propre). **L'architecture cible est désormais décidée** : VinX est un **settlement layer ZK-natif** avec consensus **PoS Algorand-style** (comité VRF n≈100, BLS agrégé) et un écosystème d'**Appchains** générant des preuves SP1.
>
> **Deux pivots architecturaux actés en août 2026 :**
> 1. **PoA Threshold → PoS Algorand** : le consensus cible est le comité VRF (ADR 0029, promu Accepté). Le pool de validateurs devient permissionless (ADR 0038). La sélection par score est remplacée par la sélection par VRF.
> 2. **Modules bondés → Appchains ZK** : les modules hors-L1 génèrent des preuves SP1 soumises au L1 (ADR 0050). Le L1 passe du niveau 1 (bond + réputation) au niveau 3 (vérification cryptographique). ForceExit (ADR 0048) et Clearinghouse (ADR 0049) complètent l'écosystème.
>
> **La source de vérité de la feuille de route, c'est [`docs/adr/README.md`](./docs/adr/README.md).** Ce document-ci décrit le **code tel qu'il tourne** ; l'index ADR décrit **ce qui est décidé et ce qui reste**.
>
> ### Implémenté dans la tranche consensus (avec ADR dédié)
> - **ADR 0002 — Finalité au quorum** : pointeur `finalized_height` prefix-closed, quorum historique par hauteur, refus de bâtir dans le vide (`MAX_UNFINALIZED_DEPTH = 64`). Éprouvé au banc n=3.
> - **ADR 0027 — Jailing (t1+t2a+t2b)** : cœur pur déterministe, état câblé dans `settle_block`, rotation leader/backup sur le set actif. Sûreté : quorum de finalité sur le set complet bondé (jamais réduit par le jailing).
> - **ADR 0031 — Fork-choice (t1+t2a+t2b)** : `canonical_head` pure + réorg snapshot+rejeu, câblé dans le handler P2P. Convergence prouvée au banc n=3.
> - **ADR 0046 — BLS12-381 agrégé** : implémentation complète (`vinx-crypto/bls.rs`), co-signatures BLS dans les blocs. Base de l'agrégation pour le comité VRF.
> - **ADR 0040 — Émission progressive** : minting pur, T_half ~20 ans, invariant garanti. Cadence fixe 12 s, 3 000 tx/bloc.
>
> ### Implémenté antérieurement
> **0015** (vérif parallèle signatures) · **0026** (dépôt existentiel) · **0022** (P2P anti-DoS) · **0011** (gouvernance K-of-M) · **0010** (registre modules bondés) · **0020** (sérialisation canonique) · **0005** (MTP) · **0043** (cadence fixe 12 s) · **0045** (abolition heartbeat).
>
> ### Architecture cible décidée — à implémenter
> **PoS Algorand (🔴 priorité haute) :** comité VRF ECVRF RFC 9381 (ADR 0029), admission permissionless PoS (ADR 0038). **Appchains ZK (🔴 priorité haute) :** SP1 proof verification L1 (ADR 0050), ForceExit/Escape Hatch (ADR 0048), Clearinghouse cross-chain (ADR 0049), Celestia DA (ADR 0034). **Économie (🟠) :** récompenses par époque (ADR 0028), rémunération modules escrow (ADR 0039). **Sûreté (🟠) :** accountability co-sign (ADR 0030), churn validateurs (ADR 0036), bornes ressources tx (ADR 0035).
>
> ### ⚠️ Chemin critique
> **Court terme :** tx `Unjail` + règle 2 co-signatures (ADR 0027) ; accountability co-sign (ADR 0030). **Moyen terme :** comité VRF (ADR 0029) — prérequis de tout le reste ; admission PoS (ADR 0038) ; récompenses époque (ADR 0028). **Long terme :** SP1 proof verification (ADR 0050) ; ForceExit (ADR 0048) ; Clearinghouse (ADR 0049) ; Celestia (ADR 0034).

---

## Table des matières

1. [Ce qu'on a construit](#1-ce-quon-a-construit)
2. [Architecture — les 5 briques](#2-architecture--les-5-briques)
3. [Le protocole en détail](#3-le-protocole-en-détail)
4. [L'API RPC (endpoints HTTP)](#4-lapi-rpc-endpoints-http)
5. [Le wallet CLI — toutes les commandes](#5-le-wallet-cli--toutes-les-commandes)
6. [Le SDK TypeScript](#6-le-sdk-typescript)
7. [Infrastructure & outils](#7-infrastructure--outils)
8. [Ce qui reste à faire](#8-ce-qui-reste-à-faire)
9. [Commandes utiles du quotidien](#9-commandes-utiles-du-quotidien)

---

## 1. Ce qu'on a construit

VinX Ledger est une blockchain L1 de paiement écrite intégralement en Rust, sans framework tiers. Voici la liste complète des fonctionnalités implémentées :

### Protocole de base
- [x] Cryptographie Ed25519 + SHA-256 + adresses Bech32 (`vinx1...`)
- [x] Arbre de Merkle avec preuves d'inclusion vérifiables
- [x] 7 types de transactions (transfer, stake, unstake, announce-upgrade, slash-validator, admin-action, anchor-state) — voir §2
- [x] État mondial (`WorldState`) avec validation complète
- [x] État de genèse configurable (admin + validateur initial)
- [x] Consensus PoA Threshold *(implémenté)* — quorum `⌈2n/3⌉` sur le **set complet bondé**, leader round-robin sur le **set actif** (jailés sautés, ADR 0027). *Architecture cible : PoS Algorand-style comité VRF n≈100 (ADR 0029, non encore implémenté).*
- [x] **Finalité au quorum** (ADR 0002) — pointeur `finalized_height` explicite, **prefix-closed**, évalué contre le **quorum historique à chaque hauteur** ; à n=1 immédiate, à n≥2 suit les co-signatures. Éprouvée au **banc n=3** (liveness / tolérance 1 panne / sûreté à 1/3)
- [x] **Refus de bâtir dans le vide** — le producteur (leader et backup) ne scelle pas au-delà de `MAX_UNFINALIZED_DEPTH` (64) blocs non finalisés (ADR 0002)
- [x] **Fork-choice déterministe câblé** (ADR 0031 t1+t2a+t2b) — `canonical_head` pure + candidats concurrents + réorg par snapshot+rejeu (`reorg`) déclenchée dans le handler P2P ; convergence indépendante de l'ordre **prouvée au banc n=3** ; *reste : soak réseau réel*
- [x] **Jailing / fiabilité des validateurs** (ADR 0027) — set actif dérivé de faits on-chain (proposeur effectif ≠ leader prévu), rotation sur le set actif ; le quorum de finalité n'est **jamais** réduit par le jailing (sûreté)

### Production de blocs
- [x] Producteur **à cadence fixe 12 s** (ADR 0043 + ADR 0045) : un bloc toutes les **12 s** exactement — l'ancienne accélération dos-à-dos (génératrice de forks) a été retirée, le heartbeat périodique de 10 min a été aboli. La congestion passe par le base-fee, pas par des blocs rapprochés. Garde anti-spin conservée.
- [x] Capacités : **3 000 tx/bloc** (~250 TPS), mempool **100 000** (réglables `max_block_txs` / `max_mempool_size`)
- [x] Frais dynamiques style EIP-1559 (×1 à ×3 selon la charge mémoire)
- [x] **Frais au producteur** : 100 % des frais du bloc créditent le validateur producteur (plus de melt)
- [x] Vérification des signatures en parallèle (rayon, tous les cœurs CPU)
- [x] Détection des slots manqués + **jailing déterministe** (proposeur effectif ≠ leader prévu → manquement attribué ; jail après `MAX_MISSED_PROPOSALS`, ADR 0027)

### Réseau P2P
- [x] Gossipsub libp2p pour la propagation des blocs et transactions
- [x] mDNS — découverte automatique des pairs en LAN
- [x] Vérification cryptographique des blocs avant application
- [x] Détection d'équivocation (double-signature sur deux blocs différents)
- [x] Synchronisation de blocs par P2P (`SyncRequest` / `SyncResponse`)
- [x] Synchronisation au démarrage depuis un pair de confiance (HTTP)

### Économie — fair launch v6 (implémenté)
- [x] Supply totale : 100 milliards de VinX, **immuable, sans burn** (18 décimales) — courbe d'émission **gravée immuable** (ADR 0021, révisé ADR 0040)
- [x] **Genèse sans pre-mine** : 0 en circulation, 0 émis — les tokens n'existent pas avant d'être produits par le travail (**ADR 0040 ✅**)
- [x] **Invariant vérifié à chaque bloc** (garde dure) : `circulating_supply + epoch_dist_emission_pot + destroyed_atoms == emitted_atoms ≤ 100 Md` (ADR 0040)
- [x] **Émission par le travail** : minting progressif (`mint_emission()`), décroissance exponentielle continue (`T_half` ~20 ans, R₀ ≈ 3,47 Md/an), intégrée sur les **timestamps**. 100 % au producteur actuellement ; **ADR 0028 (Accepté, non implémenté)** ajoutera la distribution par époque entre proposeurs et co-signataires
- [x] **Frais forfaitaires** (indépendants du montant), **100 % au producteur** (immédiatement, hors époque) ; multiplicateur de congestion ×1–3
- [x] **Dépôt existentiel + reaping** (ADR 0026) : plancher 0,001 VINX, comptes vidés supprimés de l'état

### Gouvernance — clé admin OU comité K-of-M (ADR 0011)

- [x] `AdminAction` (0x08) encapsule une `GovernanceAction`. **Sans policy installée** : une clé admin unique (legacy) exécute immédiatement. **Avec un comité K-of-M** (`SetAdminPolicy`) : une action requiert `threshold` approbations de signataires distincts (chaque approbation = une tx mono-signée)
- [x] 6 `GovernanceAction` : `AddValidator`, `RemoveValidator`, `UpdateFeeFloor`, `ScheduleUpgrade`, `RotateAdmin`, `SetAdminPolicy`
- [x] Une fois un comité installé, les raccourcis mono-admin sont désactivés (une clé isolée ne court-circuite plus le seuil). **Pas de gel de compte** (interdit par le protocole)
- [ ] *Différé* : garde-fous de gouvernance (ADR 0032, 🚧 à discuter), gouvernance par les validateurs (ADR 0011 t2)

### Sécurité
- [x] Slashing : preuve d'équivocation → 10 % bounty au rapporteur, 90 % **versés dans le pot d'époque** (`epoch_dist_emission_pot`, redistribués aux validateurs honnêtes via ADR 0028), validateur exclu
- [x] Rate limiting : 100 requêtes/minute par IP (middleware axum)
- [x] Auth token Bearer sur les routes `/snapshot` et `/admin/compact`
- [x] Compaction de chaîne (`compact_old_txs`) — supprime les tx anciennes, conserve les headers

### API & interfaces
- [x] Endpoints HTTP/REST (voir section 4)
- [x] Server-Sent Events `/events` — push en temps réel à chaque bloc
- [x] WebSocket `/ws` — identique aux SSE, protocole bidirectionnel
- [x] Métriques Prometheus sur `/metrics` (dont `vinx_remaining_supply`, `vinx_emitted_atoms`, `vinx_epoch_emission_pot`)
- [x] Interface web embarquée sur `/` (explorateur + wallet, signature locale)
- [x] **Console d'administration `/admin`** — dashboard, validateurs (ajout/retrait/approbation), upgrades, maintenance ; actions signées localement avec la clé admin, lecture seule sinon
- [x] Preuves Merkle via `/account/:address/proof`

### Robustesse & performance
- [x] **Migration de schéma forward** : les anciennes données sont migrées en place à l'ouverture — plus de wipe au changement de version (repli snapshot documenté sinon)
- [x] Démarrage **sans panique** (ouverture du stockage faillible et gracieuse)
- [x] Clés typées (`Address`/`Hash32`) + **ahash** sur les index et le mempool (hot-path sans allocation de String)
- [x] Persistance incrémentale (seuls les comptes modifiés réécrits) + Merkle incrémental

### Outillage
- [x] Wallet CLI — 24 commandes (voir section 5)
- [x] SDK TypeScript — 15 méthodes, 22 types, 19 tests Jest
- [x] Docker Compose — testnet 3 validateurs prêt à l'emploi
- [x] CI/CD GitHub Actions — 4 jobs automatiques
- [x] Script devnet local `scripts/devnet.sh`
- [x] HD wallet BIP-39 — mnémonique 12 mots → clé ed25519

---

## 2. Architecture — les 5 briques

```
crates/
├── vinx-crypto/     Primitives cryptographiques
├── vinx-core/       Types du protocole (blocs, transactions, gouvernance)
├── vinx-state/      État de la chaîne (WorldState + genèse)
├── vinx-node/       Nœud complet (P2P, consensus, RPC, mempool)
└── vinx-wallet/     Wallet CLI et client RPC
sdk/
└── vinx-sdk/        SDK TypeScript
```

### `vinx-crypto` — Cryptographie

| Élément | Description |
|---------|-------------|
| `sha256(data)` | Hachage SHA-256 |
| `KeyPair` | Paire de clés Ed25519 |
| `PublicKey` | Clé publique 32 octets |
| `VinxSignature` | Signature Ed25519 64 octets |
| `Address` | Adresse Bech32 `vinx1...` |
| `merkle_root(leaves)` | Racine d'un arbre de Merkle |
| `merkle_proof_for(leaves, index)` | Preuve d'inclusion pour la feuille n°`index` |
| `verify_merkle_proof(leaf, proof, root)` | Vérifie une preuve sans l'arbre complet |

### `vinx-core` — Protocole

| Élément | Description |
|---------|-------------|
| `Amount` | Montant en atomes (10⁻¹⁸ VinX), précision maximale |
| `Account` | Adresse, solde, staked, nonce, `stake_since` |
| `Block` / `BlockHeader` | Bloc avec hauteur, hash, validateur, frais de base |
| `BlockSignature` | Co-signature d'un validateur |
| `SlashEvidence` | Preuve de double-signature |
| `Transaction` | 7 types actifs encodés dans un objet unique (voir table plus bas) |
| `ValidatorSet` | Ensemble des validateurs autorisés + calcul quorum `⌈2n/3⌉` |
| `GovernanceAction` | Actions de gouvernance (6 variants, dont `SetAdminPolicy`) |
| `ModuleOp` | Opérations du registre de modules (Register/Anchor/Deregister — ADR 0010) |

**Les types de transactions** (`0x05`/`0x06` retirés en ADR 0007 — l'ajout/retrait de validateur passe par `AdminAction`) :

| Code | Type | Rôle |
|------|------|------|
| `0x01` | `Transfer` | Envoi de VinX |
| `0x02` | `Stake` | Verrouillage en staking (bond) |
| `0x03` | `Unstake` | Déverrouillage (file de déliaison 3 j) |
| `0x04` | `AnnounceUpgrade` | Annonce d'une mise à jour protocole (admin) |
| `0x07` | `SlashValidator` | Slashing pour équivocation (preuve vérifiée) |
| `0x08` | `AdminAction` | Action de gouvernance (`GovernanceAction`) — mono-admin ou comité K-of-M (ADR 0011) |
| `0x09` | `AnchorState` | Opération de registre de modules (`ModuleOp` : Register/Anchor/Deregister — ADR 0010) |

**Les `GovernanceAction`** (payload d'`AdminAction`) : `AddValidator`, `RemoveValidator`, `UpdateFeeFloor`, `ScheduleUpgrade`, `RotateAdmin`, **`SetAdminPolicy`** (installe le comité K-of-M — ADR 0011).

### `vinx-state` — WorldState

Le `WorldState` est l'état complet de la chaîne. Il est sérialisé sur disque après chaque bloc.

| Champ | Type | Description |
|-------|------|-------------|
| `accounts` | `BTreeMap<Address, Account>` | Tous les comptes (clé = adresse 20 octets, itération triée) |
| `circulating_supply` | `Amount` | Tokens détenus par les comptes (hors `epoch_dist_emission_pot`) |
| `block_height` | `u64` | Hauteur actuelle |
| `emitted_atoms` | `u128` | Cumul des atomes mintés depuis la genèse (seul traceur d'émission — ADR 0040) |
| `emission_epoch_ts` | `u64` | Timestamp du début de l'époque d'émission courante (référence de la courbe) |
| `pending_unbonds` | `Vec<PendingUnbond>` | Déliaisons en cours (bond, `unlock_ts`) — slashable jusqu'à maturation |
| `fee_floor` / `base_fee` | `Amount` | Plancher de frais gouvernable / frais dynamiques (congestion ×1–3) |
| `admin_address` | `Option<Address>` | Clé admin legacy (1-de-1, si aucun comité) |
| `admin_policy` | `Option<AdminPolicy>` | **Comité K-of-M** (ADR 0011) — supersède `admin_address` quand présent |
| `pending_governance` | `Vec<GovernanceProposal>` | Propositions en attente d'approbations (ADR 0011) |
| `modules` | `BTreeMap<Hash32, ModuleEntry>` | **Registre de modules bondés** (ADR 0010) |
| `current_version` / `pending_upgrade` | | Version protocole + upgrade planifié (activation par timestamp, ADR 0006) |
| `validator_set` | `ValidatorSet` | Validateurs actifs (quorum `⌈2n/3⌉`, round-robin) |
| `epoch_dist_emission_pot` | `Amount` | **✅ ADR 0040** — atomes reçus du slash 90 %, accumulés jusqu'à distribution (ADR 0028) |
| `destroyed_atoms` | `u128` | **✅ ADR 0040** — atomes définitivement perdus (reaping de comptes poussière) ; avec `epoch_pot` ferme l'invariant |
| `epoch_dist_start_ts` | `u64` | *(à venir — ADR 0028)* Timestamp de début de l'époque de distribution en cours |
| `epoch_dist_proposer_credits` | `BTreeMap<Address, u128>` | *(à venir — ADR 0028)* Crédits proposeur accumulés dans l'époque |
| `epoch_dist_cosign_counts` | `BTreeMap<Address, u32>` | *(à venir — ADR 0028)* Nombre de co-signatures par validateur dans l'époque |
| `pending_escrows` | `BTreeMap<Hash32, EscrowEntry>` | *(à venir — ADR 0039)* Escrows de paiement de modules ouverts |

### `vinx-node` — Nœud complet

Modules internes :

| Module | Rôle |
|--------|------|
| `chain` | Suivi du tip de chaîne, détection d'équivocation |
| `consensus` | Validation des blocs, signatures de co-validateurs |
| `mempool` | File de transactions (tri par frais, vérification nonce) |
| `producer` | Production de blocs, frais dynamiques, récompenses |
| `p2p` | Gossipsub + mDNS (libp2p) |
| `sync` | Synchronisation depuis un pair RPC au démarrage |
| `storage` | Persistance bincode sur disque |
| `rpc` | Serveur HTTP axum, WebSocket, SSE, rate limiting |
| `node` | Orchestration : boucle de production, events, persist |

---

## 3. Le protocole en détail

### Consensus PoA Threshold

1. Les validateurs sont listés dans le `ValidatorSet` (triés par adresse)
2. Le leader du slot `height` = `active_validators[height % n_actif]` — la rotation saute les validateurs **jailés** (ADR 0027) ; sur slot-skip, un backup produit après timeout (tout validateur enregistré peut proposer, aligné sur `validate_block`)
3. Le leader produit le bloc et le broadcast via P2P
4. Les autres validateurs co-signent le hash du header
5. **Finalité prefix-closed** (ADR 0002) : `finalized_height` avance sur le plus long préfixe contigu de blocs dont `valid_sigs ≥ quorum(hauteur)`. Le quorum est celui du **set complet bondé** à cette hauteur (`⌈2n/3⌉`, schedule `note_quorum`/`quorum_at`) — **jamais réduit par le jailing** (sûreté sous partition). À n=1 la finalité est immédiate ; à n≥2 elle suit les co-signatures P2P
6. La genèse (height 0) est toujours considérée finalisée
7. **Sûreté vérifiée au banc n=3** : à 2/3 vivant la finalité avance, à 1/3 elle **gèle** (le tip continue mais aucun bloc n'atteint le quorum) → pas de double-finalité

### Frais dynamiques (EIP-1559 adapté)

```
charge = mempool_pending / max_block_txs
multiplier = 1.0 + 2.0 × max(0, charge - 0.8) / 0.2
base_fee = fee_floor × multiplier  (cap à 3×)
```

Le multiplicateur de congestion (×1–3) est basé sur la **demande** (remplissage du mempool), pas sur la valeur transférée. Le frais est un **forfait × poids(type)**, indépendant du montant, et va **100 % au producteur**. Le multiplicateur s'applique au `base_fee` gouvernable courant.

### Gouvernance — clé admin OU comité K-of-M (ADR 0011)

On-chain, une `GovernanceAction` est soumise via `AdminAction`. **Sans comité installé**, une clé admin unique l'exécute immédiatement (legacy 1-de-1). **Avec un comité K-of-M** (`SetAdminPolicy`), l'action s'exécute une fois qu'elle a réuni `threshold` approbations de signataires distincts — chaque approbation reste une **tx mono-signée** (aucun changement du format `Transaction`). Une action rejetée ne consomme pas de nonce (validation avant mutation). Pas de gel de compte.

Actions disponibles :

| Action | Effet |
|--------|-------|
| `AddValidator(addr)` | Ajoute un validateur (bond requis) |
| `RemoveValidator(addr)` | Retire un validateur (refus du dernier) |
| `UpdateFeeFloor { atoms }` | Modifie le plancher de frais |
| `ScheduleUpgrade { version, activation_ts }` | Planifie un upgrade (timestamp, ADR 0006) |
| `RotateAdmin(addr)` | Change la clé admin legacy |
| `SetAdminPolicy { signers, threshold }` | Installe/remplace le comité K-of-M (ADR 0011) |

### Émission par le travail (fair launch v6 — ADR 0040 ✅)

> Les tokens **n'existent pas avant d'être produits**. À chaque bloc, le nœud appelle `mint_emission()` qui calcule l'émission accumulée depuis le dernier bloc (formule continue `R₀·e^(−λΔt)`, `T_half` ~20 ans), incrémente `emitted_atoms`, crédite le producteur et met à jour `circulating_supply`. Aucune réserve pré-allouée — le cumul `emitted_atoms` est la seule source de vérité. `remaining_supply = MAX_SUPPLY − emitted_atoms` est dérivé à la demande.
>
> L'invariant **garanti à chaque bloc** : `circulating_supply + epoch_dist_emission_pot + destroyed_atoms == emitted_atoms ≤ MAX_SUPPLY`.
>
> *ADR 0028 (Accepté, non implémenté) :* l'émission va aujourd'hui **100 % au producteur**. ADR 0028 la distribuera **par époque** (1 h par défaut) : `PROPOSER_SHARE_BPS` (20 % indicatif) revient aux proposeurs, le reste est partagé entre co-signataires proportionnellement à leurs co-signatures dans l'époque. Le **slash 90 %** dort dans `epoch_dist_emission_pot` jusqu'à ce qu'ADR 0028 distribue. Les **frais** restent au producteur immédiatement, hors époque. À `n=1`, comportement identique.

---

## 4. L'API RPC (endpoints HTTP)

Le nœud expose un serveur HTTP sur `0.0.0.0:8545` par défaut.

| Méthode | Route | Description |
|---------|-------|-------------|
| GET | `/` | Interface web (explorateur de blocs) |
| GET | `/admin` | Console d'administration (dashboard + gouvernance signée en local) |
| GET | `/health` | Santé du nœud : hauteur, mempool, statut |
| GET | `/chain/height` | Hauteur actuelle du tip |
| GET | `/chain/sync?from=N&limit=N` | Synchronisation d'une plage de blocs |
| GET | `/block/:height` | Bloc complet par hauteur |
| GET | `/tx/:hash` | Transaction par hash hex |
| GET | `/account/:address` | Solde, nonce, staked d'un compte |
| GET | `/account/:address/txs?limit=N&offset=N` | Historique des transactions |
| GET | `/account/:address/proof` | Preuve Merkle d'inclusion dans l'état |
| POST | `/tx/submit` | Soumettre une transaction signée |
| GET | `/mempool/size` | Nombre de transactions en attente |
| GET | `/validators` | Ensemble des validateurs actifs + quorum |
| GET | `/protocol/version` | Version courante + upgrade planifiée |
| GET | `/network/stats` | Statistiques économiques (base_fee, remaining_supply, emitted_atoms, epoch_pot, admin) |
| GET | `/metrics` | Métriques Prometheus |
| GET | `/events` | Server-Sent Events — push par bloc |
| GET | `/ws` | WebSocket — push par bloc |
| GET | `/snapshot` | État complet JSON (⚠ admin auth si token configuré) |
| POST | `/admin/compact` | Supprime les vieilles tx des blocs (⚠ admin auth) |
**Auth admin :** si `admin_token` est configuré dans `config.toml`, les routes `/snapshot` et `/admin/compact` requièrent le header `Authorization: Bearer <token>`.

**Rate limiting :** 100 requêtes / 60 secondes par IP. Retourne HTTP 429 en cas de dépassement.

---

## 5. Le wallet CLI — toutes les commandes

```bash
cargo run -p vinx-wallet -- <commande> [options]
```

**Gestion des clés :**

| Commande | Options clés | Description |
|----------|-------------|-------------|
| `keygen` | `--output wallet.json` | Génère une nouvelle paire de clés |
| `new-wallet` | `--output wallet.json` | Génère un wallet HD (mnémonique BIP-39 12 mots) |
| `restore-wallet` | `--output wallet.json` | Restaure un wallet depuis un mnémonique |
| `address` | `--wallet wallet.json` | Affiche l'adresse du wallet |

**Consultation :**

| Commande | Options clés | Description |
|----------|-------------|-------------|
| `balance` | `--address vinx1...` | Solde d'un compte |
| `block` | `--height N` | Détails d'un bloc |
| `tx` | `--hash abc123` | Détails d'une transaction |
| `status` | | Hauteur de chaîne + mempool |
| `history` | `--address vinx1... --limit 20` | Historique des transactions |
| `validators` | | Liste des validateurs actifs |
| `protocol` | | Version protocole + upgrade planifiée |
| `network-stats` | | Frais de base, pools, supply |

**Transactions :**

| Commande | Options clés | Description |
|----------|-------------|-------------|
| `transfer` | `--to vinx1... --amount 100` | Envoyer des VinX |
| `stake` | `--amount 500` | Verrouiller des VinX en staking |
| `unstake` | `--amount 500` | Déverrouiller du staking |

**Admin — transactions directes (requiert la clé admin) :**

| Commande | Options clés | Description |
|----------|-------------|-------------|
| `add-validator` | `--validator vinx1...` | Ajouter un validateur |
| `remove-validator` | `--validator vinx1...` | Retirer un validateur |
| `announce-upgrade` | `--version 1.1.0 --activation-ts 1793000000` | Planifier un upgrade (timestamp Unix, ADR 0006) |

**Admin — actions de gouvernance (requiert la clé admin) :**

| Commande | Options clés | Description |
|----------|-------------|-------------|
| `admin-action` | `--action '{"AddValidator":"vinx1..."}'` | Exécute une `GovernanceAction` immédiatement |

Exemple d'actions JSON valides :
```bash
# Ajouter un validateur
--action '{"AddValidator":"vinx1abc..."}'

# Retirer un validateur
--action '{"RemoveValidator":"vinx1abc..."}'

# Modifier le plancher de frais (en atomes)
--action '{"UpdateFeeFloor":{"atoms":100000000000000}}'

# Faire tourner la clé admin
--action '{"RotateAdmin":"vinx1nouveau..."}'
```

> Toutes les commandes de transaction acceptent `--node http://127.0.0.1:8545` (défaut) et `--wallet wallet.json` (défaut).

---

## 6. Le SDK TypeScript

**Installation :**
```bash
cd sdk/vinx-sdk && npm install && npm run build
```

**Utilisation :**
```typescript
import VinxClient from '@vinx/sdk';

const client = new VinxClient('http://localhost:8545');

// Santé du nœud
const health = await client.health();

// Solde d'un compte
const account = await client.account('vinx1abc...');

// Envoyer une transaction
const result = await client.submitTx({ ... });

// Stream temps réel (SSE)
const es = client.blockEvents();
es.onmessage = (e) => console.log(JSON.parse(e.data));
```

**Méthodes disponibles :**

| Méthode | Description |
|---------|-------------|
| `health()` | Santé du nœud |
| `height()` | Hauteur de chaîne |
| `account(address)` | Détails d'un compte |
| `accountTxs(address, opts?)` | Historique des transactions |
| `block(height)` | Bloc par hauteur |
| `tx(hash)` | Transaction par hash |
| `submitTx(tx)` | Soumettre une transaction |
| `mempool()` | Taille du mempool |
| `validators()` | Validateurs actifs |
| `protocolStatus()` | Version protocole |
| `networkStats()` | Statistiques économiques |
| `chainSync(from, limit?)` | Synchronisation de blocs |
| `proposals()` | Liste des propositions |
| `proposal(id)` | Proposition par ID |
| `blockEvents()` | Stream SSE temps réel |

**Tests :** 19 tests Jest couvrant chaque méthode + gestion d'erreurs.

---

## 7. Infrastructure & outils

### Docker Compose (3 validateurs)

```bash
docker compose up --build
```

Lance 3 nœuds en réseau isolé :
- **node1** : `localhost:8545` (RPC) + `localhost:9001` (P2P) — nœud genesis
- **node2** : `localhost:8546` (RPC) + `localhost:9002` (P2P) — sync depuis node1
- **node3** : `localhost:8547` (RPC) + `localhost:9003` (P2P) — sync depuis node1

### Devnet local (sans Docker)

```bash
./scripts/devnet.sh           # démarre 3 nœuds en local
./scripts/devnet.sh --clean   # repart de zéro
```

### CI/CD GitHub Actions

4 jobs se déclenchent à chaque push / PR :

| Job | Ce qu'il vérifie |
|-----|-----------------|
| `cargo test` | ~309 tests unitaires et d'intégration (workspace) |
| `clippy` | Qualité du code Rust (zéro warning autorisé) |
| `rustfmt` | Formatage du code |
| `sdk-test` | 19 tests TypeScript Jest |

### Configuration du nœud (`config.toml`)

```toml
block_time_secs = 12          # ADR 0043 : plancher fixe ; au plus un bloc / 12 s (pas de dos-à-dos)
max_block_txs = 10000         # Transactions max par bloc
max_mempool_size = 100000     # Transactions max en attente dans le mempool
rpc_listen = "0.0.0.0:8545"  # Adresse RPC
data_dir = "data"             # Répertoire des données
admin_token = "secret"        # Token Bearer pour les routes admin (optionnel)

p2p_listen = "/ip4/0.0.0.0/tcp/9000"   # Activer le P2P
peers = ["/ip4/1.2.3.4/tcp/9000"]       # Pairs de démarrage
sync_peer_rpc = "http://1.2.3.4:8545"  # Sync depuis un pair au démarrage
```

---

## 8. Ce qui reste à faire

> **La feuille de route détaillée vit dans [`docs/adr/README.md`](./docs/adr/README.md)** (chaque item a un ADR avec statut, contexte, décision, alternatives). Ci-dessous, la vue d'ensemble priorisée.

| Priorité | Chantier | ADR |
|----------|----------|-----|
| 🔴 **Haute** | **Jailing — finir** : tx `Unjail` (opérateur, après cooldown) + règle 2 (co-signatures absentes) | 0027 |
| 🔴 Haute | **Accountability co-sign** (détection des co-signatures conflictuelles → finalité *accountable*) ; reste de la finalité (view-change formel) | 0030, 0002 |
| 🔴 Haute | **Soak fork-choice n=3** sur réseau réel (convergence prouvée au banc ; reste la validation réseau) | 0031 |
| 🔴 Haute | **Comité VRF Algorand-style** — ECVRF RFC 9381, sélection uniforme parmi les bondés, leader = VRF le plus faible, finalité BFT ≥ 67 % du comité ; n=100 | 0029 (Accepté) |
| 🔴 Haute | **Open PoS** — admission permissionless par bond, warmup 3 époques, rotation époque top-N par score, bond gouvernable dans bornes immuables | 0038 (Accepté) |
| 🔴 Haute | **SP1 proof verification L1** — vérification Groth16 on-chain des preuves ZK Appchain, `AnchorState` étendu, deux modes (Bonded / ZK-verified) | 0050 (Accepté) |
| 🔴 Haute | **ForceExit / Escape Hatch** — tx `0x0B` + MerkleProof solde Appchain, timeout 10 blocs, slash séquenceur si ignoré | 0048 (Accepté) |
| 🔴 Haute | **Clearinghouse cross-Appchain** — LOCK → CrossMsg L1 → MINT → ACK, timeout + remboursement | 0049 (Accepté) |
| 🔴 Haute | **Celestia DA** — engagement `DaCommitment` dans `AnchorState`, blobs publiés sur Celestia V1 | 0034 (Accepté) |
| 🟠 Moyenne | **Récompenses par époque** — distribuer `epoch_dist_emission_pot` + émission entre proposeurs + co-signataires (1 h, `PROPOSER_SHARE_BPS=20 %`) ; frais restent immédiats au producteur | 0028 (Accepté) |
| 🟠 Moyenne | **Rémunération des modules par escrow** — `ModuleEscrow`/`ModuleEscrowRefund`, partage via `fee_schedule`, preuve de livraison via `AnchorState` | 0039 (Accepté) |
| 🟠 Moyenne | **Bootstrap §1** (genèse multi-validateurs + `genesis_hash`), **garde-fous de gouvernance** (🚧 à discuter), **bornes de churn** & **de ressources par tx** | 0033§1, 0032, 0036, 0035 |
| 🟠 Moyenne | **Slashing de fraude** (prérequis DA Celestia) | 0023 |
| 🟠 Moyenne | **Tokenomics Appchains** : répartition par usage/melt, époque de règlement, garde-fous d'équité, émission élastique | 0041, 0042, 0044, 0047 |
| 🟢 Future | **Blocs compacts**, light client, rent d'état, clés HSM, halt d'urgence, TLS natif, post-quantique, SLO | 0037, 0014, 0013, 0012, 0017, 0019, 0016, 0018 |

### Déjà fait (historique)

- **Tranche consensus multi-validateur + ADR 0040 (août 2026)** : banc n=3 multi-process réel (`scripts/bench-n3.sh`), finalité au quorum + quorum historique par hauteur + refus de bâtir dans le vide (0002), jailing t1/t2a/t2b — rotation sur set actif, quorum de finalité gardé sur le set complet pour la sûreté (0027), fork-choice `canonical_head` pure + câblage de bout en bout + réorg snapshot+rejeu (0031 t1+t2a+t2b), horloge protocole sur MTP (0005), **cadence fixe 12 s + abolition heartbeat** (ADR 0045). **ADR 0040** : émission progressive sans La Fonderie — `mint_emission()`, `emitted_atoms`, `epoch_dist_emission_pot`, `destroyed_atoms`, `EMISSION_T_HALF_SECS` (~20 ans) ; cadence **12 s** fixe, **3 000 tx/bloc** max (~250 TPS). Migration stockage **v9→v10→v11** (~309 tests ✅). Plus un lot **sécurité** : routes admin fail-closed + constant-time, sync HTTP vérifiée, keystore wallet chiffré (argon2 + AES-GCM), rate-limiter borné, persistance incrémentale de la chaîne par hauteur.
- **Durcissement & extensions** : vérif parallèle des signatures (0015), dépôt existentiel + reaping (0026), durcissement P2P anti-DoS (0022), gouvernance K-of-M (0011), registre de modules bondés (0010), vecteurs dorés canoniques (0020). *Plus* « cohérence & robustesse » : chain_id sûr (0008), immutabilité d'émission (0021), temps réseau (0005), frais stake/unstake (0009), unification gouvernance (0007), préavis upgrade temps réel (0006).
- **Refonte économique fair launch v6** : genèse sans pre-mine, émission par le travail (T_half ~20 ans), frais forfaitaires au producteur, bond de validateur + déliaison temps réel, slashing réparé (vérification cryptographique réelle).
- **Itérations plus anciennes** : console admin `/admin`, robustesse au démarrage, clés typées + ahash, capacités relevées (10k tx/bloc, mempool 100k), cadence de bloc adaptative, migration de schéma sans wipe.

### 🧭 Reprendre le travail (prochaine session)

**Architecture cible décidée en août 2026 :** VinX = settlement layer ZK-natif, consensus PoS Algorand-style (comité VRF n≈100, BLS agrégé), Appchains ZK (SP1 + Celestia DA).

Le **socle L1 est solide** (banc n=3, 309 tests). Les prochains chantiers par priorité :
1. **(Court terme)** tx `Unjail` + règle 2 co-signatures absentes (ADR 0027) ; accountability co-sign (ADR 0030) ; soak fork-choice n=3 réseau réel.
2. **(Moyen terme — PoS Algorand)** Comité VRF ECVRF RFC 9381 (ADR 0029) — **prérequis de tout le reste** ; admission PoS permissionless (ADR 0038) ; récompenses par époque (ADR 0028).
3. **(Long terme — Appchains ZK)** SP1 proof verification L1 (ADR 0050) ; ForceExit (ADR 0048) ; Clearinghouse (ADR 0049) ; Celestia DA (ADR 0034).

---

## 9. Commandes utiles du quotidien

```bash
# Lancer le devnet local
./scripts/devnet.sh

# Générer un wallet
cargo run -p vinx-wallet -- new-wallet --output mon-wallet.json

# Vérifier le solde
cargo run -p vinx-wallet -- balance --address vinx1...

# Envoyer des VinX
cargo run -p vinx-wallet -- transfer \
  --wallet mon-wallet.json \
  --to vinx1... \
  --amount 1000

# Voir les propositions de gouvernance
cargo run -p vinx-wallet -- show-proposals

# Lancer les tests
cargo test --workspace

# Construire le projet
cargo build --release

# Lancer avec Docker
docker compose up --build
```
