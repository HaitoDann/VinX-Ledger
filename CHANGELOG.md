# VinX Ledger — Changelog

Suivi rétrospectif de toutes les versions du projet.  
Format : `MAJEUR.MINEUR.CORRECTIF` — les versions `0.x.y` sont des versions de développement pré-production.

---

## [0.11.0] — 2026-06-12

### Gouvernance admin-only & distribution pool

Refonte fondamentale du modèle de gouvernance suite à la décision de design : VinX Labs conserve le contrôle exclusif du protocole on-chain. Suppression complète du système de vote entre validateurs.

**Gouvernance**
- Suppression du système de propositions et de vote on-chain (`SubmitProposal` 0x0B, `VoteProposal` 0x0C)
- Remplacement par `AdminAction` (0x0B) : encapsule une `GovernanceAction`, requiert la signature admin, s'exécute immédiatement
- Les propositions et sondages communautaires se font hors-chaîne (Discord, forum)

**Pool de distribution**
- Renommage `ReleaseMeltToStaking` → `ReleaseMeltToDistribution`
- Ajout du champ `distribution_pool` dans `WorldState` : réceptacle du melt redistribué et du Coffre Maturité débloqué
- `UnlockCoffre` libère vers `distribution_pool` (pas `staking_pool`)
- Le melt n'est pas un burn : les tokens restent en circulation dans le pool de distribution

**Corrections supply**
- Rétablissement des constantes correctes : 21 millions VinX (admin) + ~99,979 milliards (Coffre Maturité) = 100 milliards max
- Correction de `storage_test.rs` qui contenait une valeur erronée (21 milliards au lieu de 21 millions)

**SDK TypeScript**
- Suppression des types `ProposalResponse`, `ProposalListResponse` et des méthodes `proposals()` / `proposal()` (routes `/governance/proposals` supprimées)
- Ajout de `distribution_pool` dans `NetworkStatsResponse`

**Infrastructure**
- Fusion de la branche `claude/awesome-gauss-IMII2` dans `main` (13 commits)
- Suppression des doublons de tests introduits par le merge (`make_block`, tests Merkle)
- 42 tests Rust + 17 tests SDK Jest — zéro échec

---

## [0.10.0] — 2026-06-12

### WebSocket, rotation admin & Coffre Maturité

**Temps réel**
- Endpoint WebSocket `/ws` : push des événements de bloc en temps réel (complément des SSE `/events`)
- Gestion des connexions multiples simultanées

**Sécurité admin**
- Auth token Bearer sur `/snapshot` et `/admin/compact` (champ `admin_token` dans `NodeConfig`)
- Rotation de la clé admin via `AdminAction::RotateAdmin` sans redémarrage du nœud

**Coffre Maturité**
- Ajout des 3 conditions d'unlock : `MicaCasp`, `ExternalAudit`, `PublicPolicy`
- `GovernanceAction::MarkCoffreCondition` : marque une condition comme remplie
- `GovernanceAction::UnlockCoffre` : refuse si les 3 conditions ne sont pas toutes vraies
- Champs `coffre_mica_casp`, `coffre_external_audit`, `coffre_public_policy` dans `WorldState`

**Tests**
- Test d'intégration multi-validateur : `test_admin_action_adds_validator`

---

## [0.9.0] — 2026-06-12

### Frais dynamiques, slashing, HD wallet, SDK TypeScript & CI

**Économie & frais**
- Frais dynamiques style EIP-1559 : multiplicateur ×1 à ×3 selon la charge du mempool (>80% → surge)
- Répartition des frais 40 % staking / 30 % validateur / 30 % melt pool
- Fee floor configurable, `base_fee` recalculé à chaque bloc
- File d'attente mempool triée par priorité de frais (highest-fee-first)

**Sécurité réseau**
- Slashing : preuve d'équivocation (`SlashEvidence`) → 10 % bounty au rapporteur, 90 % vers melt pool, validateur exclu du set
- Vérification des signatures en parallèle via `rayon` (tous les cœurs CPU disponibles)
- Pipeline de staging parallèle pour la validation des tx en mempool
- Rate limiting HTTP : 100 requêtes/minute par IP (middleware Axum `ConnectInfo`)

**Cryptographie**
- HD wallet BIP-39 : mnémonique 12 mots → seed → clé Ed25519
- Preuves Merkle d'inclusion : `merkle_proof_for`, `verify_merkle_proof`, `MerkleProofStep`
- Endpoint `/account/:address/proof` exposant la preuve en JSON

**Réseau P2P**
- mDNS : découverte automatique des pairs en LAN sans configuration
- Validation cryptographique des blocs avant application P2P
- Détection d'équivocation sur les blocs reçus par P2P

**SDK TypeScript** (`sdk/vinx-sdk/`)
- Package npm complet avec TypeScript + Jest
- Client `VinxClient` : 13 méthodes typées couvrant tous les endpoints
- 17 tests Jest couvrant tous les appels RPC
- Types : `AccountResponse`, `BlockResponse`, `TxResponse`, `ValidatorSetResponse`, `NetworkStatsResponse`, etc.

**CI/CD**
- GitHub Actions : 4 jobs (`test`, `clippy`, `fmt`, `sdk-test`)
- Script `scripts/devnet.sh` : lance un devnet 3 nœuds local avec flag `--clean`

---

## [0.8.0] — 2026-06-12

### Phase testnet : sync, Docker, métriques & SSE

**Synchronisation**
- Ordre strict des transactions par nonce dans le mempool (rejection si nonce incorrect)
- Synchronisation de blocs au démarrage depuis un pair de confiance (`--sync-peer` HTTP)
- Endpoint GET `/chain/sync?from=N&limit=L` pour la synchronisation légère
- Historique des transactions par compte (`/account/:address/txs`) avec pagination

**Infrastructure**
- `docker-compose.yml` : 3 nœuds + volumes persistants
- Script `scripts/setup-testnet.sh` pour initialiser un testnet multi-machine
- Compaction de chaîne : `POST /admin/compact` supprime les tx anciennes, conserve les headers
- Endpoint `GET /snapshot` : export JSON complet de l'état (state + tip hash)

**Observabilité**
- Métriques Prometheus sur `GET /metrics` : hauteur, supply, staking pool, melt pool, transactions
- Endpoint `GET /network/stats` : base_fee, pools économiques, supply circulante

**Temps réel**
- Server-Sent Events `GET /events` : push de `{"type":"new_block","height":N}` à chaque bloc
- Reconnexion automatique côté client

**Validateurs dynamiques**
- `GovernanceAction::AddValidator` / `RemoveValidator` : modifie le set actif sans redémarrage
- Vérification complète des blocs P2P avant application (hash, signatures, quorum)

---

## [0.7.0] — 2026-06-11

### P2P gossipsub, gouvernance admin & versioning protocole

**P2P libp2p**
- Gossipsub pour la propagation des blocs et transactions entre pairs
- Configuration via `--p2p-listen` et `--peers` (CLI + config TOML)
- Module `vinx-node/src/p2p.rs` intégré dans la boucle tokio

**Administration**
- Transactions `FreezeAccount` (0x04) et `UnfreezeAccount` (0x05) : gel judiciaire de compte
- Gel automatique après 365 jours × 10 s/bloc (3 153 600 blocs)
- Vérification que l'émetteur est bien la clé admin dans `WorldState`

**Versioning protocole**
- `ProtocolVersion { major, minor, patch }` dans `WorldState`
- Transaction `AnnounceUpgrade` (0x06) : annonce avec délai minimal obligatoire selon le type (patch/minor/major)
- `ScheduledUpgrade` : activation automatique à la hauteur annoncée

**RPC & CLI**
- Lookup de transaction par hash : `GET /tx/:hash`
- Wallet : commandes admin (`freeze`, `unfreeze`, `announce-upgrade`, `admin-action`)
- Configuration TOML `config.toml` : tous les paramètres CLI disponibles en fichier
- UI web enrichie : affichage de l'état de gel, versions protocole, validateurs

---

## [0.6.0] — 2026-06-10

### PoA Threshold & Merkle state root

**Consensus PoA Threshold**
- Quorum ⌈2n/3⌉ : un bloc est finalisé quand suffisamment de validateurs l'ont co-signé
- Round-robin leader : chaque validateur produit à son tour selon `height % n`
- Finalité déterministe et immédiate (pas de réorganisation)
- `ValidatorSet` : liste des validateurs actifs, calcul du quorum, vérification des signatures

**Merkle state root**
- `compute_state_root()` : hash Merkle de tous les comptes (triés par adresse)
- `hash_account()` : sérialisation canonique d'un compte en bytes
- Inclus dans chaque `BlockHeader` (`state_root: Hash32`)
- Preuves d'inclusion vérifiables par les clients légers

**Persistence**
- `Storage::new(path)` / `save(&state, &chain)` / `load()` : sérialisation bincode sur disque
- Reprise automatique au redémarrage depuis le dernier état persisté
- Test `test_state_persists_across_node_restarts` : valide la continuité sur 5 blocs

---

## [0.5.0] — 2026-06-09

### Tokenomics refactor & whitepaper v2.4

**Tokenomics**
- Supply totale fixée à 100 milliards de VinX (précision 18 décimales)
- Constantes immuables : `MAX_SUPPLY_ATOMS`, `ADMIN_ALLOCATION_ATOMS`, `COFFRE_MATURITY_ATOMS`
- Fee floor par défaut : 0,0001 VinX ; constantes `FEE_NUMERATOR` / `FEE_DENOMINATOR` (0,05 %)
- Staking rewards distribués toutes les `STAKING_DISTRIBUTION_INTERVAL` blocs (100 blocs)
- Stake minimum : 1 VinX (`MIN_STAKE_ATOMS`)

**Documentation**
- Whitepaper v2.3 : décision de stack custom Rust (abandon de Substrate)
- Whitepaper v2.4 : passage de FBA à PoA Threshold comme algorithme de consensus
- README mis à jour avec la vue d'ensemble du projet

---

## [0.4.0] — 2026-06-03

### Interface web & récompenses de staking

**Interface web**
- Dashboard HTML/CSS/JS embarqué dans le binaire Rust (`rpc/ui.rs`)
- Explorateur de blocs : navigation prev/next/latest, liste des transactions
- Lookup de compte : solde, nonce, statut
- Envoi de transactions depuis le navigateur (signature Ed25519 via TweetNaCl — clé privée jamais transmise au serveur)
- Tabs Transfer / Stake / Unstake avec estimation de frais
- Affichage des validateurs et du quorum
- Mises à jour temps réel via SSE
- Thème sombre, responsive

**Staking**
- Distribution proportionnelle des récompenses à tous les stakers à chaque bloc
- Vérification du stake minimum

**Correctifs**
- Compatibilité Windows pour le wallet CLI (séparateur `--` obligatoire)
- Affichage d'un message clair si un compte n'existe pas (404 → message UI)

---

## [0.3.0] — 2026-05-25

### Implémentation initiale complète — 114 tests

Premier état complet et fonctionnel du projet. Toutes les briques de base opérationnelles de bout en bout.

**Protocole**
- 7 types de transactions : `Transfer`, `Stake`, `Unstake`, `FreezeAccount`, `UnfreezeAccount`, `Emission`, `AddValidator`
- `WorldState` : gestion des comptes, balances, nonces, staking
- Validation complète des transactions (signature Ed25519, nonce, solde suffisant)
- `Chain` : séquence de blocs avec index par hauteur, hash tip
- Genesis state configurable (`create_genesis_state`, `GenesisConfig`)

**Réseau**
- Nœud HTTP RPC (`vinx-node`) avec Axum
- Endpoints : `/health`, `/chain/height`, `/account/:address`, `/block/:height`, `/mempool/size`, `/validators`, `/tx/submit`
- Mempool avec validation et déduplication
- Producteur de blocs asynchrone (tokio), intervalle configurable

**Wallet CLI**
- `keygen` : génération de paire de clés Ed25519 + adresse Bech32 `vinx1...`
- `balance` : consultation du solde
- `transfer` : envoi de VinX signé
- `stake` / `unstake` : gestion du staking
- `block` : consultation d'un bloc par hauteur

**Tests**
- 114 tests unitaires et d'intégration couvrant : crypto, transactions, state, chain, RPC HTTP

**Documentation**
- Guide de démarrage pas à pas

---

## [0.2.0] — 2026-05-23

### Workspace Rust & crates fondamentaux

Mise en place de l'architecture multi-crates.

**`vinx-crypto`**
- Ed25519 (dalek) : `KeyPair`, `PublicKey`, `Signature`
- SHA-256 : `sha256()`
- Arbre de Merkle : `merkle_root()`
- Adresses Bech32 `vinx1...` : encodage/décodage
- `Hash32` : alias `[u8; 32]`

**`vinx-core`**
- `Amount` : entier u128 en atomes (10⁻¹⁸ VinX), arithmetic checked
- `Transaction` : structure signée avec `signing_bytes()` et `hash()`
- `Block` / `BlockHeader` : avec hash déterministe
- `Account` : balance, nonce, staked, frozen
- `CoreError` : erreurs typées
- `ValidatorSet` : placeholder initial

**`vinx-state`**
- `WorldState` : état mutable du registre
- `apply_transaction()` : dispatch par type de tx
- Tests unitaires sur chaque type de transaction

---

## [0.1.0] — 2026-05-21

### Cadrage & initialisation

- Initialisation du dépôt GitHub `HaitoDann/VinX-Ledger`
- Document de cadrage : définition du projet VinX Ledger (blockchain L1 de paiement)
- Choix technologiques initiaux : Rust, Ed25519, Bech32, PoA
- `.gitignore` pour les artefacts Rust

---

*Ce fichier est maintenu manuellement à chaque sprint. Pour voir le détail d'un commit : `git show <hash>`.*
