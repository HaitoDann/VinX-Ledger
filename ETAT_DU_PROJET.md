# VinX Ledger — État du Projet

> Document de référence interne — mis à jour à chaque sprint.
> Dernière mise à jour : **août 2026** (**consensus multi-validateur éprouvé : banc n=3 réel, finalité au quorum, jailing/rotation, fork-choice câblé de bout en bout**). Base économique implémentée : fair launch v5 (halving 8 ans, whitepaper v4.0). **Direction économique décidée mais non implémentée :** émission élastique à réservoir + garde-fous d'équité + écosystème de subnets (ADR 0039/0040/0041/**0042 époque**/**0044 équité & amorçage**). **Cadence de consensus révisée** (ADR 0043 : block time 12 s fixe, sans accélération — anti-fork).

---

> ## 🧭 Où en est le projet (lis ceci en premier)
>
> **Le socle est solide et le consensus multi-validateur est désormais éprouvé en réel.** Le **banc n=3 multi-process** (`scripts/bench-n3.sh`) valide, avec de vraies co-signatures P2P : liveness (finalité en lockstep), tolérance à 1 panne (2/3 finalise), et **sûreté** (à 1/3 la finalité gèle, le tip continue). Sur ce socle : fair launch v5, état anti-bloat, P2P durci, gouvernance K-of-M, modules bondés. `cargo test --workspace` **vert** (~309 tests), clippy `-D warnings` & fmt propres. Stockage **schéma v11**.
>
> **La source de vérité de la feuille de route, c'est [`docs/adr/README.md`](./docs/adr/README.md)** — l'index de tous les ADR (Décisions d'Architecture), avec pour chacun son statut (✅ implémenté / Proposé / 🚧 brouillon). Ce document-ci décrit le **code tel qu'il tourne** ; l'index ADR décrit **ce qui est décidé et ce qui reste**.
>
> ### Implémenté dans la tranche consensus (avec ADR dédié)
> - **ADR 0002 — Finalité au quorum** : pointeur `finalized_height` explicite, **prefix-closed**, évalué contre le **quorum historique à chaque hauteur** (`Chain::note_quorum`/`quorum_at`, corrige le blocage du préfixe après changement de set) ; avancé aussi sur les chemins de sync (P2P + HTTP). **+ Refus de bâtir dans le vide** (`MAX_UNFINALIZED_DEPTH = 64`).
> - **ADR 0027 — Jailing (t1+t2a+t2b)** : cœur pur déterministe (`vinx-core::reliability`), état câblé dans `settle_block` (`WorldState.reliability`, migration meta **v10→v11**), et **rotation** leader/backup sur le **set actif** (jailés sautés). ⚠️ **Sûreté :** le quorum de finalité **reste sur le set complet bondé** — jamais réduit par le jailing (le banc a montré que le réduire casse la sûreté sous partition ; seule la gouvernance réduit `n`).
> - **ADR 0031 — Fork-choice (t1 + t2a + t2b)** : `consensus::canonical_head` (fonction pure) **câblé de bout en bout** — candidats concurrents, mécanisme de réorg par snapshot+rejeu (`reorg`), et déclenchement vivant dans le handler P2P. **Convergence indépendante de l'ordre d'arrivée prouvée au banc n=3.** Reste : soak multi-nœuds réseau réel.
> - **ADR 0005 — Horloge protocole sur MTP** : émission/déliaison/upgrade comparent au Median Time Past incluant le bloc, sur les 3 chemins (prod/P2P/sync).
> - **ADR 0038 — Heartbeat 10 min** : au moins un bloc toutes les 600 s, supprime l'incitation à forcer des blocs par fausses tx.
>
> ### Implémenté antérieurement (durcissement & extensions)
> **0015** (vérif parallèle des signatures) · **0026** (dépôt existentiel + reaping) · **0022** (durcissement P2P anti-DoS) · **0011** (gouvernance K-of-M) · **0010** (registre de modules bondés) · **0020** (vecteurs dorés canoniques). Plus un lot sécurité : routes admin fail-closed + comparaison constant-time, sync HTTP vérifiée (proposeur/quorum/state_root), keystore wallet chiffré (argon2 + AES-GCM), rate-limiter borné, persistance incrémentale de la chaîne par hauteur.
>
> ### Décidé mais NON implémenté — la prochaine grande direction
> **Pivot économique & écosystème** (ADR rédigés, à implémenter) : **0040** émission élastique `E = r·F` (r=7 %, melt→Fonderie, remplace le halving — amende 0021), **0041** répartition de l'émission entre subnets par l'usage/melt (rejette le staking à la TAO), **0039** infrastructure de subnets (escrow bondé + racine de récompense + Claim), **0042** l'époque de règlement (fenêtre MTP, robuste au bloc-on-demand), **0044** garde-fous d'équité & amorçage (émission plafonnée par l'usage `min(r·F·Δt, k·M)` → tue le jackpot de démarrage à froid ; canal unique demand-pull « valeur = ce qui est payé » ; deux rails paiement/melt ; pont valeur-externe « Qubic » Montage 2 ; seed float genesis). Objectif : des développeurs créent des subnets à vraie boucle économique, les mineurs gagnent des VINX par un service réel. Idées de subnets : [`docs/subnets/CATALOGUE.md`](docs/subnets/CATALOGUE.md) (4 retenus : monitoring, stockage, calcul/build+Qubic, annotation IA). Le simulateur `scripts/emission_sim.py` a servi à choisir r=7 % (à réutiliser pour caler `k`/CAP/bond).
>
> ### Autres propositions ouvertes — voir l'index ADR
> Consensus/sûreté : **0030** (accountability co-sign), **0036** (churn validateurs), reste de **0002** (view-change) et **0031** (wiring reorg). Économie/lancement : **0028** (partage d'émission), **0033** (bootstrap fair-launch). Modules : **0034** (DA & preuve d'ancre), **0023** (slashing de fraude). Gouvernance : **0032** (garde-fous, 🚧). Scaling : **0029** (BLS + comité VRF), **0035** (bornes ressources), **0037** (blocs compacts). Divers : **0012** (clés HSM), **0013** (rent d'état), **0014** (light client), **0016** (post-quantique), **0017** (halt), **0018** (SLO), **0019** (TLS).
>
> ### ⚠️ Le chemin critique
> Le banc n=3 et le **wiring reorg du fork-choice (0031 t2b)** sont **faits** (convergence prouvée au banc n=3). **Chemin critique désormais : le pivot économique constitutionnel** — figer puis implémenter 0040 (+ garde-fous 0044) → 0041 → 0042 (époque) → 0039 (subnets), gated par des décisions **immuables** à trancher avant mainnet. En parallèle (faible risque) : tx `Unjail` (0027), règle 2 (co-signatures absentes), **0030** (accountability), et le **soak fork-choice n=3 sur réseau réel**.

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
- [x] Consensus PoA Threshold — quorum `⌈2n/3⌉` sur le **set complet bondé**, leader round-robin sur le **set actif** (jailés sautés, ADR 0027)
- [x] **Finalité au quorum** (ADR 0002) — pointeur `finalized_height` explicite, **prefix-closed**, évalué contre le **quorum historique à chaque hauteur** ; à n=1 immédiate, à n≥2 suit les co-signatures. Éprouvée au **banc n=3** (liveness / tolérance 1 panne / sûreté à 1/3)
- [x] **Refus de bâtir dans le vide** — le producteur (leader et backup) ne scelle pas au-delà de `MAX_UNFINALIZED_DEPTH` (64) blocs non finalisés (ADR 0002)
- [x] **Fork-choice déterministe câblé** (ADR 0031 t1+t2a+t2b) — `canonical_head` pure + candidats concurrents + réorg par snapshot+rejeu (`reorg`) déclenchée dans le handler P2P ; convergence indépendante de l'ordre **prouvée au banc n=3** ; *reste : soak réseau réel*
- [x] **Jailing / fiabilité des validateurs** (ADR 0027) — set actif dérivé de faits on-chain (proposeur effectif ≠ leader prévu), rotation sur le set actif ; le quorum de finalité n'est **jamais** réduit par le jailing (sûreté)

### Production de blocs
- [x] Producteur **à plancher de cadence fixe** (ADR 0043) : au plus un bloc toutes les **12 s**, même en saturation (l'ancienne accélération dos-à-dos, génératrice de forks, a été retirée — la congestion passe par le base-fee). Skip-empty au repos conservé. Garde anti-spin (pas de blocs vides en boucle sur backlog inapplicable)
- [x] **Heartbeat 10 min** (ADR 0038) : au repos, au moins un bloc (même vide) toutes les 600 s — forge l'émission accumulée à heure fixe (supprime l'incitation à forcer des blocs par fausses tx), borne le retard du MTP, fait mûrir déliaisons/upgrades. Coût : ~15-30 Mo/an
- [x] Capacités : **10 000 tx/bloc**, mempool **100 000** (réglables `max_block_txs` / `max_mempool_size`)
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

### Économie — fair launch v5 (implémenté)
- [x] Supply totale : 100 milliards de VinX, **immuable, sans burn** (18 décimales) — courbe d'émission **gravée immuable** (ADR 0021)
- [x] **Genèse sans pre-mine** : 0 en circulation, 100 Md scellés dans **La Fonderie**
- [x] **Invariant vérifié à chaque bloc** (garde dure) : `circulating_supply + foundry == 100 Md` (ADR 0004)
- [x] **Émission par le travail** : La Fonderie se vide *uniquement* pour rémunérer la production, décroissance par **halving 8 ans** (arithmétique entière déterministe), intégrée sur les **timestamps**. Créditée au producteur, **non pondérée par le bond**
- [x] **Frais forfaitaires** (indépendants du montant), **100 % au producteur** (plus de melt) ; multiplicateur de congestion ×1–3
- [x] **Dépôt existentiel + reaping** (ADR 0026) : plancher 0,001 VINX, comptes vidés supprimés de l'état

### Gouvernance — clé admin OU comité K-of-M (ADR 0011)

- [x] `AdminAction` (0x08) encapsule une `GovernanceAction`. **Sans policy installée** : une clé admin unique (legacy) exécute immédiatement. **Avec un comité K-of-M** (`SetAdminPolicy`) : une action requiert `threshold` approbations de signataires distincts (chaque approbation = une tx mono-signée)
- [x] 6 `GovernanceAction` : `AddValidator`, `RemoveValidator`, `UpdateFeeFloor`, `ScheduleUpgrade`, `RotateAdmin`, `SetAdminPolicy`
- [x] Une fois un comité installé, les raccourcis mono-admin sont désactivés (une clé isolée ne court-circuite plus le seuil). **Pas de gel de compte** (interdit par le protocole)
- [ ] *Différé* : garde-fous de gouvernance (ADR 0032, 🚧 à discuter), gouvernance par les validateurs (ADR 0011 t2)

### Sécurité
- [x] Slashing : preuve d'équivocation → 10 % bounty au rapporteur, 90 % **fondu dans La Fonderie**, validateur exclu
- [x] Rate limiting : 100 requêtes/minute par IP (middleware axum)
- [x] Auth token Bearer sur les routes `/snapshot` et `/admin/compact`
- [x] Compaction de chaîne (`compact_old_txs`) — supprime les tx anciennes, conserve les headers

### API & interfaces
- [x] Endpoints HTTP/REST (voir section 4)
- [x] Server-Sent Events `/events` — push en temps réel à chaque bloc
- [x] WebSocket `/ws` — identique aux SSE, protocole bidirectionnel
- [x] Métriques Prometheus sur `/metrics` (dont `vinx_foundry`)
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
| `circulating_supply` | `Amount` | Tokens détenus par les comptes (= `MAX_SUPPLY - foundry`) |
| `block_height` | `u64` | Hauteur actuelle |
| `foundry` | `Amount` | **La Fonderie** — réserve d'émission (fair launch). `circulation + foundry == 100 Md`, gardé à chaque bloc |
| `emission_epoch_ts` / `emitted_atoms` | `u64` / `u128` | Époque d'émission + cumul émis (suivi de la courbe halving) |
| `pending_unbonds` | `Vec<PendingUnbond>` | Déliaisons en cours (bond, `unlock_ts`) — slashable jusqu'à maturation |
| `fee_floor` / `base_fee` | `Amount` | Plancher de frais gouvernable / frais dynamiques (congestion ×1–3) |
| `admin_address` | `Option<Address>` | Clé admin legacy (1-de-1, si aucun comité) |
| `admin_policy` | `Option<AdminPolicy>` | **Comité K-of-M** (ADR 0011) — supersède `admin_address` quand présent |
| `pending_governance` | `Vec<GovernanceProposal>` | Propositions en attente d'approbations (ADR 0011) |
| `modules` | `BTreeMap<Hash32, ModuleEntry>` | **Registre de modules bondés** (ADR 0010) |
| `current_version` / `pending_upgrade` | | Version protocole + upgrade planifié (activation par timestamp, ADR 0006) |
| `validator_set` | `ValidatorSet` | Validateurs actifs (quorum `⌈2n/3⌉`, round-robin) |

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

### Émission par le travail (fair launch)

> La Fonderie se vide **uniquement** pour rémunérer la production de blocs, selon une décroissance par **halving tous les 8 ans** (`débit(t) = R₀·2^(−t/8 ans)`), **intégrée sur les timestamps** — une chaîne inactive ne produit aucun bloc (donc rien n'est forgé), et le premier bloc après une période d'activité forge l'émission accumulée depuis le précédent. La Fonderie ne se recharge jamais (aucun melt) — elle décroît de façon monotone jusqu'à la poussière, puis c'est **fees-only**. L'invariant `circulation + Fonderie = 100 Md` reste vrai trivialement.
>
> *Note d'incitation (voir ADR 0028) :* l'émission va aujourd'hui **100 % au producteur** ; l'ADR 0028 (proposé) la partagerait entre le proposeur et les co-signataires du quorum, pour rémunérer la finalité et lisser la distribution.
>
> *Direction actée (non implémentée) :* le halving ci-dessus est **remplacé** par l'émission élastique à réservoir (0040, `E=r·F`) **plafonnée par l'usage** (`min(r·F·Δt, k·M)`, 0044) ; les validateurs vivent alors des **frais**, l'émission finance les **subnets** par l'usage/melt (0041/0039), réglée par **époque** (0042). Voir 0044 pour les garde-fous d'équité (jackpot à froid, canal unique, deux rails, amorçage).
>
> **⚠️ Évolution décidée (non implémentée) — ADR 0040/0041/0039 :** le halving discret sera remplacé par une **émission élastique à réservoir** `E = r · F` (fraction `r = 7 %` de la Fonderie, le **melt** recyclant vers la Fonderie → auto-régulation vers `C* = MAX − M/r`). Cette émission alimentera des **reward pools de subnets**, répartis **au prorata du VINX melté (usage réel)** par subnet, et les subnets seront hébergés par une infrastructure d'**escrow bondé + racine de récompense + Claim par preuve Merkle** (0039). Les validateurs vivront alors des **frais**. Le code décrit ci-dessus (halving 8 ans) reste ce qui tourne aujourd'hui.

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
| GET | `/network/stats` | Statistiques économiques (base_fee, foundry, supply, admin) |
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
block_time_secs = 5           # Écart entre blocs sous activité légère ; se resserre à charge, dos à dos à saturation
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
| 🔴 **Haute** | **Fork-choice — wiring reorg** : stocker les candidats concurrents + réorg bornée sous finalité (la fonction pure `canonical_head` est faite). À valider via le banc n=3 (déjà en place). | 0031 t2 |
| 🔴 Haute | **Jailing — finir** : tx `Unjail` (opérateur, après cooldown) + règle 2 (co-signatures absentes) | 0027 |
| 🔴 Haute | **Accountability co-sign** (détection des co-signatures conflictuelles → finalité *accountable*) ; reste de la finalité (view-change formel) | 0030, 0002 |
| 🟣 **Pivot éco** | **Émission élastique** `E=r·F` (remplace le halving, amende 0021) → **répartition par usage/melt** entre subnets → **infrastructure de subnets** (escrow + reward root + Claim). La vraie proposition de valeur. | 0040 → 0041 → 0039 |
| 🟠 Moyenne | **Partage d'émission sur le quorum**, **bootstrap fair-launch multi-validateur** | 0028, 0033 |
| 🟠 Moyenne | **Garde-fous de gouvernance** (🚧), **bornes de churn** & **de ressources par tx** | 0032, 0036, 0035 |
| 🟠 Moyenne | **Modules : DA & preuve d'ancre** puis **slashing de fraude** (prérequis des subnets) | 0034, 0023 |
| 🟢 Future | **Décentralisation à l'échelle** (BLS + comité VRF), **blocs compacts** | 0029, 0037 |
| 🟢 Future | Light client (0014), rent d'état (0013), clés HSM (0012), halt d'urgence (0017), TLS natif (0019), post-quantique (0016), SLO (0018) | — |

### Déjà fait (historique)

- **Tranche consensus multi-validateur (dernière en date)** : banc n=3 multi-process réel (`scripts/bench-n3.sh`), finalité au quorum + quorum historique par hauteur + refus de bâtir dans le vide (0002), jailing t1/t2a/t2b — rotation sur set actif, quorum de finalité gardé sur le set complet pour la sûreté (0027), fork-choice `canonical_head` pure (0031 t1), horloge protocole sur MTP (0005), heartbeat 10 min (0038). Migration stockage **v10→v11**. Plus un lot **sécurité** : routes admin fail-closed + constant-time, sync HTTP vérifiée, keystore wallet chiffré (argon2 + AES-GCM), rate-limiter borné, persistance incrémentale de la chaîne par hauteur.
- **Durcissement & extensions** : vérif parallèle des signatures (0015), dépôt existentiel + reaping (0026), durcissement P2P anti-DoS (0022), gouvernance K-of-M (0011), registre de modules bondés (0010), vecteurs dorés canoniques (0020). *Plus* « cohérence & robustesse » : chain_id sûr (0008), immutabilité d'émission (0021), temps réseau (0005), frais stake/unstake (0009), unification gouvernance (0007), préavis upgrade temps réel (0006).
- **Refonte économique fair launch v5** : genèse sans pre-mine, émission par le travail (halving 8 ans), frais forfaitaires au producteur, bond de validateur + déliaison temps réel, slashing réparé (vérification cryptographique réelle).
- **Itérations plus anciennes** : console admin `/admin`, robustesse au démarrage, clés typées + ahash, capacités relevées (10k tx/bloc, mempool 100k), cadence de bloc adaptative, migration de schéma sans wipe.

### 🧭 Reprendre le travail (prochaine session)

Le prochain chantier consensus est le **wiring reorg du fork-choice (ADR 0031 t2)** : aujourd'hui l'acceptation est « premier-vu » ; il faut stocker les blocs concurrents à une même hauteur et basculer sur `canonical_head` sous la finalité (borné par `MAX_UNFINALIZED_DEPTH`). Approche : incrémentale et **re-validée via le banc n=3 (déjà en place)** à chaque étape (le consensus est fork-critique). Alternative si tu préfères avancer l'économie : démarrer l'implémentation de l'**émission élastique (0040)** — c'est constitutionnel (amende 0021, met à jour le test-tripwire) mais bien spécifié.

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
