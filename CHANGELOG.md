# VinX Ledger — Changelog

Suivi rétrospectif de toutes les versions du projet.  
Format : `MAJEUR.MINEUR.CORRECTIF` — les versions `0.x.y` sont des versions de développement pré-production.

---

## [0.19.0] — 2026-06-30

### Incremental Merkle tree — O(log n) state root par bloc

**`IncrementalMerkleTree` — `vinx-crypto`**
- Nouvelle structure qui stocke tous les niveaux de l'arbre en mémoire (niveaux `0` = feuilles, `k` = nœuds internes, racine = `levels.last()[0]`)
- `build(leaves)` : construction initiale O(n)
- `update_leaf(idx, hash)` : mise à jour d'une feuille et propagation vers la racine en O(log n) — seuls les nœuds du chemin affecté sont recalculés
- `rebuild(leaves)` : reconstruction complète O(n) — utilisée uniquement lors de l'ajout d'un nouveau compte
- `leaves()` : expose le tableau de feuilles trié, compatible avec `merkle_proof_for`
- 6 tests unitaires couvrant : racine vide, cohérence avec `merkle_root` (n = 1..10), mise à jour ponctuelle, mise à jour exhaustive, reconstruction, compatibilité des preuves d'inclusion

**`WorldState` — `vinx-state`**
- Remplacement de `cached_root: Option<Hash32>` par trois champs `#[serde(skip)]` : `merkle_tree: IncrementalMerkleTree`, `leaf_index: HashMap<String, usize>`, `dirty_addrs: HashSet<String>` et `needs_rebuild: bool`
- `mark_dirty(addr)` : remplace `invalidate_root_cache()` dans tous les `apply_*`, `credit()`, `check_auto_unfreeze()` et `distribute_staking_rewards()` ; détecte automatiquement les nouveaux comptes et active `needs_rebuild`
- `flush_dirty()` : si `needs_rebuild` → `full_rebuild()` O(n) ; sinon → `update_leaf()` pour chaque adresse sale O(|dirty| × log n)
- `compute_state_root()` : appelle `flush_dirty()` puis retourne `merkle_tree.root()` en O(1)
- **Gain mesuré** : pour un bloc de 100 tx touchant 200 comptes sur 10 000 : O(200 × 14) ≈ 2 800 ops contre O(10 000 × 14) = 140 000 ops auparavant → ~47× plus rapide sur ce scénario

---

## [0.18.0] — 2026-06-30

### Métriques P2P câblées & persistance du mempool

**Métriques P2P — Compteurs désormais actifs**
- `p2p::start()` accepte désormais `NodeMetrics` en paramètre et le transmet à la boucle d'événements
- `p2p_blocks_recv` incrémenté à chaque bloc validé et appliqué via gossip P2P
- `p2p_tx_recv` incrémenté à chaque transaction reçue via gossip P2P
- Les métriques sont partagées sans verrou entre le nœud, le rate limiter et la couche P2P

**Persistance du mempool (optimisation #3)**
- `Storage::serialize_mempool` / `save_mempool_blob` / `load_mempool` : nouvelles méthodes pour sérialiser, compresser (zstd niveau 3) et restaurer le mempool en une transaction redb dédiée
- `Node::persist()` inclut désormais le mempool dans chaque cycle de sauvegarde sur disque, sans rallonger la durée de verrouillage (sérialisation sous verrou lecture, I/O hors verrou)
- Au démarrage (`main.rs`), les transactions persistées sont rechargées et réinjectées dans le mempool en appliquant la validation complète — les transactions expirées ou invalides sont silencieusement ignorées
- Nombre de transactions restaurées affiché dans les logs (`tracing::info!`)

---

## [0.17.0] — 2026-06-30

### Rate limiting token bucket, métriques atomiques & WebSocket enrichi

**Rate limiting — Token bucket par route**
- Remplacement du compteur à fenêtre fixe par un token bucket (burst autorisé + refill continu)
- Limites différenciées par type de route :
  - `/health`, `/metrics`, `/ws`, `/events` : exempt (aucune limite)
  - `/tx/submit` : 20 req burst, refill 20/min
  - `/faucet/*` : 5 req burst, refill 5/heure
  - Tout le reste : 100 req burst, refill 100/min
- Chaque IP dispose de buckets indépendants par classe de route
- Le nombre de requêtes rejetées est compté dans `NodeMetrics.ratelimit_hit`

**Métriques Prometheus — Compteurs atomiques temps réel**
- `NodeMetrics` : struct avec 8 champs `AtomicU64` sur le `Node`, partagée avec le `RateLimiter` via `Arc`
- `GET /metrics` lit les compteurs d'activité sans aucun verrou (lock-free) ; seules les données économiques nécessitent encore un verrou
- 8 nouvelles métriques s'ajoutent aux 8 existantes :
  - `vinx_blocks_produced_total` — blocs produits par ce nœud
  - `vinx_tx_submitted_total{status="ok"|"err"}` — tx acceptées / rejetées via RPC
  - `vinx_tx_in_block_total` — tx incluses dans les blocs produits
  - `vinx_p2p_blocks_received_total` — blocs reçus via P2P gossip
  - `vinx_p2p_tx_received_total` — transactions reçues via P2P gossip
  - `vinx_ratelimit_hit_total` — requêtes rejetées par le rate limiter
  - `vinx_last_block_timestamp_seconds` — timestamp Unix du dernier bloc produit
- Les compteurs sont incrémentés aux sites d'événement : `tick()`, `submit_tx`, P2P handlers

**WebSocket — Ping/pong keepalive & événements enrichis**
- `BlockEvent` étendu avec `base_fee_atoms`, `proposer` (adresse bech32 du validateur), `state_root` (hex)
- Handler `/ws` réécrit avec `tokio::select!` sur trois branches :
  - Réception d'un `BlockEvent` → envoi JSON enrichi
  - Timer 30 s → envoi d'un `Ping` pour détecter les connexions silencieusement fermées
  - Message entrant → gestion du `Close` et des frames Pong (ignorées)
- SSE `/events` enrichi avec les mêmes champs supplémentaires

---

## [0.16.0] — 2026-06-30

### Optimisations de performance — pruning, compression, cache & mempool

**Élagage de chaîne (block pruning)**
- Constantes `BLOCK_RETENTION_COUNT = 100 000` et `PRUNE_INTERVAL = 1 000` dans `amount.rs`
- `Chain::prune(keep_last)` : vide les listes de transactions et de signatures des blocs anciens, conserve les headers
- `Chain::compact_old_txs(keep_last)` : version légère (transactions uniquement)
- Appel automatique toutes les 1 000 blocs pendant la production

**Merkle root paresseux**
- `WorldState` porte un cache `Option<Hash32>` (`#[serde(skip)]`)
- `compute_state_root()` retourne la valeur mise en cache ; le cache est invalidé uniquement aux mutations réelles : `credit()`, `apply_transaction()`, `check_auto_unfreeze()`, `distribute_staking_rewards()`
- Élimine les recalculs O(n log n) redondants sur chaque lecture RPC

**Compression zstd**
- Tous les blobs redb (state, chain, tx-index) compressés en niveau 3 à l'écriture
- Messages P2P ≥ 512 octets compressés en niveau 1 avec un octet de flag (`0x00` = brut, `0x01` = zstd)
- Réduction estimée : 60-75 % sur l'état sérialisé, 40-60 % sur les messages gossip

**Filtre de Bloom P2P**
- `bloomfilter::Bloom<Hash32>` (taux FP 1 % à 2× capacité) pré-filtre les transactions dupliquées dans `stage()`
- Évite la vérification de signature coûteuse sur les tx déjà connues
- Correctness garantie par le `HashSet<Hash32>` exact (`seen`)

**Persistance de l'index de transactions**
- `tx_index` et `account_tx_index` sérialisés avec l'état à chaque cycle (schéma v3)
- Restauration en O(1) via `import_tx_indexes()` au démarrage ; `rebuild_tx_index()` O(n) en fallback

**Persistance asynchrone**
- Sérialisation sous verrou lecture, I/O disque entièrement hors verrou via `tokio::task::spawn_blocking`
- `Storage::serialize()` (pur, sans I/O) + `Storage::save_serialized()` (I/O seul, thread-safe)
- `Storage` devient `Clone` via `Arc<Database>`

**Vérification de signatures P2P en parallèle**
- Handlers `NewBlock` et `SyncResponse` vérifient toutes les signatures de validateurs via `rayon::par_iter()`
- Chaque worker vérifie `Address::from_public_key == validator` puis la signature Ed25519
- Débit proportionnel au nombre de cœurs CPU

**Cache LRU**
- `lru::LruCache<String, Address>` (1 024 entrées) pour le décodage bech32 — `Node::parse_address()`
- `lru::LruCache<String, TxReceipt>` (100 000 entrées) pour les reçus de transaction — handler `GET /tx/:hash`

**Validation de nonce dans le mempool**
- `min_nonce: HashMap<String, u64>` : nonce minimum acceptable par émetteur, mis à jour après chaque bloc via `update_confirmed_nonces()`
- `add()` rejette immédiatement les nonces périmés (`MempoolError::StaleNonce`) avant toute vérification de signature
- Remplacement au même nonce autorisé uniquement si le frais est strictement supérieur (fee-bump)
- `MempoolError::NonceTaken` si le slot est occupé avec un frais égal ou supérieur
- `update_confirmed_nonces()` purge également les entrées périmées de la queue et reconstruit le filtre de Bloom

**Éviction par frais**
- `try_evict_for()` : à capacité maximale, une tx à frais élevé peut évincer la tx de plus faible frais dans toutes les queues
- Tx avec frais égal ou inférieur rejetée avec `MempoolError::Full`

---

## [0.15.0] — 2026-06-29

### Block-on-demand, heartbeat conditionnel & Δ1-Δ4

**Block-on-demand**
- Le producteur de blocs ne tourne plus sur un intervalle fixe de 3 s
- `tokio::select!` entre `tx_ready.notified()` (nouvelle tx admise) et `sleep(heartbeat)`
- Fenêtre de batch 200 ms après le signal tx pour regrouper les soumissions simultanées
- `Mempool::tx_ready: Arc<Notify>` — signal déclenché à chaque `add()` et `flush_staged()`
- Constantes : `HEARTBEAT_INTERVAL_SECS = 3_600`, `BATCH_WINDOW_MS = 200`
- Économie estimée ~90 % d'espace disque en période creuse (24 blocs/jour au lieu de 28 800)

**Heartbeat conditionnel**
- Heartbeat horaire ignoré si le mempool est vide et qu'aucun timer protocolaire n'est actif
- Zéro bloc vide produit en période d'inactivité totale

**Δ1 — Répartition des frais 80/20**
- Nouveau modèle : 80 % pour le validateur (`VALIDATOR_FEE_BPS = 8000`), 20 % vers le treasury
- Champ `treasury` ajouté dans `WorldState`
- `treasury_balance()` : accesseur dédié
- Remplacement de la répartition tripartite (staking/validateur/melt) par le modèle 80/20

**Δ2 — Transactions sponsorisées (gasless UX)**
- Champs optionnels `sponsor`, `sponsor_pub_key`, `sponsor_signature` dans `Transaction`
- `with_sponsor(keypair)` : builder method
- `signing_bytes()` intègre l'adresse sponsor pour le binding cryptographique
- `apply_transfer()` : montant débité au sender, frais débités au sponsor si présent
- Validation de la clé/adresse/signature sponsor dans `apply_transaction()`

**Δ3 — Format wire Borsh pour P2P**
- `BorshSerialize` / `BorshDeserialize` sur : `Address`, `PublicKey`, `VinxSignature`, `Amount`, `Transaction`, `Block`, `BlockHeader`, `BlockSignature`, `P2pMessage`
- `P2pMessage::encode()` / `decode()` basculent de bincode vers Borsh

**Δ4 — Transport QUIC**
- Feature `quic` activée sur libp2p dans `vinx-node`
- Le Swarm écoute sur un port QUIC (UDP) en plus du port TCP

---

## [0.14.0] — 2026-06-24

### Robustesse protocole (A-E)

**A — TTL & replay protection**
- Champ `expires_at_height` dans `Transaction` : TTL par hauteur de bloc
- Champ `chain_id` dans `Transaction` : protection contre le replay cross-réseau
- `signing_bytes()` intègre `chain_id` (4 B BE) et `expires_at_height` (8 B BE)
- Builders : `with_chain_id()`, `with_expiry()`
- `receipts_root` dans `BlockHeader` : SHA-256 des hashes des tx incluses

**B — Persistence redb**
- Remplacement de bincode + fichiers plats par redb (KV embarqué ACID en Rust pur)
- 2 tables : `state` (world_state, chain) et `meta` (schema_version)
- Garde de version de schéma : données obsolètes rejetées au démarrage

**C — Chain IDs**
- Module `chain_id.rs` : `MAINNET = 1`, `TESTNET = 7`, `DEVNET = 42`
- `WorldState.chain_id` avec default serde
- `GenesisConfig.chain_id` câblé à la genèse
- Flag CLI `--chain-id` dans `vinx-node`

**D — Bootstrap P2P & réputation des pairs**
- `bootstrap_peers` dans `NodeConfig` : dialés au démarrage P2P
- `peer_reputation: HashMap<PeerId, i32>` dans la boucle d'événements
- Messages gossip invalides décrémentent le score ; bannissement à -5 via `blacklist_peer`
- Flag CLI `--bootstrap-peers`

**E — Expérience développeur**
- `GET /tx/estimate` : `base_fee`, `recommended_fee`, pression mempool
- `GET /tx/:hash/receipt` : `TxReceipt` avec succès/erreur par tx
- `TxReceipt` stocké dans `Node.receipts` à chaque tick
- `Mempool::prune_expired(height)` : supprime les tx expirées à chaque tick
- Rate limiting par adresse dans le mempool : 50 tx pendantes max par émetteur

---

## [0.13.0] — 2026-06-17

### Validateur set dynamique

**Slot skip**
- `ValidatorSet::index_of()` + `leader_idx_at()` pour le calcul du leader de secours
- `validate_block()` assoupli : tout validateur enregistré peut proposer (slot skip sans violation PoA)
- `produce_block_backup()` dans `producer.rs`
- `Node::tick_as_backup()` + `try_backup_production()` : activation échelonnée (backup à distance 1 après 2×block_time, distance 2 après 3×block_time, etc.)

**Liveness tracking**
- `Node::last_block_instant` : horloge réinitialisée à chaque bloc produit
- `Node::validator_liveness: HashMap<addr, last_height>` : mis à jour dans `after_block_produced()`
- `GET /validators` enrichi : statut online/offline, `last_seen_height`, `is_next_leader`, `next_leader`

**Inscription à la validation**
- `Node::validator_requests` : queue en mémoire des demandes d'adhésion
- `POST /validators/request` : soumettre une demande (adresse + multiaddr)
- `GET /validators/pending` : liste admin-only des demandes en attente

**Tests**
- 3 nouveaux tests : `test_validator_join_request`, `test_validator_liveness_tracking`, `test_add_validator_updates_set`
- 212 tests — 0 échec

---

## [0.12.0] — 2026-06-13

### Faucet, TLS, light client Merkle, Grafana & pipeline de release

**Faucet testnet**
- `POST /faucet/request` : envoi automatique de VinX à une adresse, cooldown 24 h par adresse
- Configuration via `config.toml` : montant, cooldown, clé faucet
- Test d'intégration `test_faucet_endpoint`

**Infrastructure TLS**
- Reverse proxy Caddy : certificat auto-signé local (devnet), Let's Encrypt automatique (VPS)
- Gestion CORS, keep-alive SSE/WebSocket, redirection HTTP → HTTPS

**Light client Merkle**
- `verifyMerkleProof(leafHash, proof, stateRoot)` côté navigateur via `crypto.subtle` (SHA-256 natif)
- Intégré dans le SDK TypeScript et l'interface web

**Tests de robustesse**
- Test `test_crash_recovery` : redémarrage du nœud + vérification de la continuité de l'état
- 11 invariants proptest : conservation des frais, déterminisme Merkle, conservation de la supply, incrément de nonce sur transfert, rejet nonce incorrect

**Observabilité**
- Stack Grafana/Prometheus : dashboard auto-provisionné, scrape 15 s
- Panneaux : hauteur de chaîne, taille mempool, base fee, soldes des pools dans le temps

**Pipeline de release**
- `.github/workflows/release.yml` : tag `vX.Y.Z` → build, tests, création de release GitHub
- Guide de démarrage rapide `GETTING_STARTED.md`
- 207 tests — 0 échec

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
