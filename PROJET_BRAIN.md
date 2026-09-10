# VinX Ledger — Project Brain
> Rail de paiement L1 minimaliste, auditable, sans VM · VinX Labs · v0.1-alpha

---

## 🌟 North Star

**Ce qu'on construit** : Un rail de paiement blockchain — envoyer des VinX rapidement, de façon fiable et vérifiable, point. Pas une plateforme de contrats intelligents, pas un L1 généraliste. La VM et les modules sont hors scope jusqu'à nouvel ordre.

**Pour qui** : Dans un premier temps, les validateurs du testnet et les développeurs qui intègrent VinX comme couche de règlement. À terme, tout utilisateur qui veut payer en VinX.

**Pourquoi maintenant** : Le protocole core est complet (consensus PoA + BLS, VRF, compact blocks, snapshot sync, parallel sync). On est à la frontière entre « code qui tourne en dev » et « réseau qui tourne en public ». C'est le moment de franchir ce seuil, pas d'ajouter des features.

**Succès à 6 mois** : Testnet actif avec 3+ validateurs indépendants, bloc produit toutes les 6 s sans interruption depuis 30 jours, wallet fonctionnel, faucet public, au moins une intégration externe (paiement ou pont) en test.

---

## 📍 Où j'en suis

> ⬆️ **Mis à jour à chaque fin de session. C'est la première chose à lire au retour.**

```
[10/09/2026 — série de durcissement post-audit]
✅ Fait :     Trois audits (Claude/ChatGPT/Gemini) → correctifs vérifiés, chacun avec PoC échouant/vert
              ADR 0069 — BLAKE3 remplace SHA-256 IMPLÉMENTÉ (avant genesis, breaking) — STORAGE_VERSION 19→20
              ADR 0070 — Auth proposeur + registre BLS indexé
              ADR 0071 — Verrou de vote persistant (Storage::claim_vote)
              ADR 0072 — consensus_root engage tout l'état (emission_started + foundry ajoutés)
              ADR 0073 — signing_bytes injectif (payload_len u32 BE) — corrige collision txid VINX-12
              ADR 0074 — Checkpoints de subjectivité faible + garde transport via crate url
              ADR 0075 — Clé BLS obligatoire au bonding + PoP liée à identité (DST V1→V2)
              ADR 0080 — Banc adversarial multi-nœuds (6 scénarios, mutation testing)
              check_admin fail-closed (VINX-20) ; passe auto-adversariale → 4 défauts trouvés
              Doc + ADR balayés et alignés (BLAKE3, PoP V2, BLS-au-bonding, recentrage v6.0)
              429 tests verts, clippy propre, tout intégré sur main

🔜 Next :     Cérémonie de genèse multi-validateurs (ADR 0075 §1) — chaque validateur fournit clé BLS + PoP
              Recoupement multi-pairs des checkpoints (ADR 0074, partiel)
              Contre-audits externes (prompts prêts) + publication SECURITY.md (ADR 0078)
              Remplir genesis-testnet.json avec les vraies adresses

🤖 AI :       Demandé → auditer, écrire les PoC, corriger, rédiger les ADR manquants pour lancer
              Codé → correctifs 0069-0080, banc adversarial, balayage documentaire complet
              ⚠️ Divergence → —

🚧 Bloqué :   Besoin des adresses admin + validateur réelles + cérémonie multi-validateurs

[02/09/2026 — ~2h]
✅ Fait :     Décision produit formalisée : VinX = rail de paiement uniquement, abandon ZK (ADR 0064)
              ADR 0064 — VinX rail de paiement (supersède modules/ZK/Appchains)
              ADR 0065 — Allocateur mimalloc (10-20% gratuits)
              ADR 0066 — Hash tx mémoïsé via OnceLock (élimine re-hashes)
              ADR 0067 — Sig cache mempool→bloc (5-10x validation bloc)
              ADR 0068 — Batch verify Ed25519 (2x vérification résiduelle)
              ADR 0069 — BLAKE3 remplace SHA-256 (⚠️ avant genesis seulement)
              Tuto Windows testnet (HTML interactif, publié artifact)

🔜 Next :     Remplir genesis-testnet.json avec les vraies adresses (admin + validateur)
              Implémenter ADR 0069 (BLAKE3) AVANT le genesis — breaking change
              Lancer le nœud de genèse sur serveur public + HTTPS

🤖 AI :       Demandé → évaluation de 17 optimisations + ADRs pour celles retenues
              Codé → 6 ADRs dans docs/adr/ (0064-0069), mise à jour PROJET_BRAIN.md
              ⚠️ Divergence → —

🚧 Bloqué :   Besoin des adresses admin + validateur réelles pour finaliser genesis-testnet.json
```

---

## ⚡ Focus maintenant

> **3 items maximum.** Tout le reste attend dans le Backlog.

- [x] ~~Implémenter ADR 0069 (BLAKE3)~~ — **fait** (10/09/2026, avant genesis)
- [ ] Cérémonie de genèse multi-validateurs (ADR 0075 §1) + remplir `genesis-testnet.json`
- [ ] Déployer le nœud de genèse sur serveur public + HTTPS

---

## 📋 Backlog vivant

### Up next *(les 5 suivantes, ordonnées par priorité)*
- Monitoring : Prometheus + Grafana sur `/metrics` — alerte si bloc stall > 2× block_time
- Faucet public accessible depuis l'interface web (déjà codé, à exposer)
- Documentation opérateur : "Rejoindre le testnet en 10 minutes" (README ou GETTING_STARTED.md)
- Script de vérification genesis : compare le hash du bloc 0 entre pairs et échoue si divergence
- Wallet web/mobile minimaliste pour le testnet (balance, send, historique)

### Optimisations validées — à implémenter post-genesis
- ADR 0065 : allocateur mimalloc (4 lignes, 10-20% throughput)
- ADR 0066 : hash tx mémoïsé (OnceLock, zéro impact protocole)
- ADR 0067 : sig cache mempool→bloc (5-10x validation bloc)
- ADR 0068 : batch verify Ed25519 (2x vérification résiduelle)

### Someday / Maybe *(idées capturées sans engagement)*
- Bridge Ethereum → VinX (wrapped VinX sur Ethereum, rédemption sur VinX)
- Modules de transaction supplémentaires (si besoin prouvé post-testnet, pas avant)
- Explorer blockchain public hébergé (aujourd'hui disponible via `/block/:height` + `/metrics`)
- SDK JavaScript pour intégrations tierces
- Programme de validateurs testnet (incentives, documentation, onboarding)
- Exécution parallèle de txs (VinxScheduler) — post-testnet seulement, détection de conflits requise
- Layout comptes plat (cache locality WorldState) — utile à partir de ~100k comptes

---

## 🧠 Décisions techniques (ADR)

> Format : `Contexte → Choix → Pourquoi → Implémenté dans`
> Règle : un ADR ne se modifie pas — créer ADR-XXX-v2 ou réviser avec un nouveau.

---

### ADR-PROD-001 : VinX = rail de paiement uniquement
Date : 26/08/2026 | Statut : **Décidé**

**Contexte** : Question posée — doit-on viser un L1 généraliste avec VM, ou un rail de paiement pur ?
**Choix** : Rail de paiement uniquement. Modules et VM hors scope jusqu'à preuve de besoin réel post-testnet.
**Pourquoi** : Surface d'attaque réduite, auditabilité totale, message produit clair, performances prévisibles, pas de comparaison directe avec Ethereum/Solana.
**Implémenté dans** : Aucun code à ajouter — c'est une contrainte de périmètre, pas une feature.
**Note** : Les types de tx actuels (Transfer, Bond, Unbond, ValidatorJoin/Exit, FeeAdjust) couvrent le cas d'usage. De nouveaux types peuvent être ajoutés sans VM si besoin prouvé.

---

### ADR-TECH-037 : Compact blocks (propagation allégée)
Date : ~08/2026 | Statut : **Implémenté**

**Contexte** : Diffuser un bloc complet (header + txs) à chaque pair consomme de la bande passante inutilement si les pairs ont déjà les txs dans leur mempool.
**Choix** : Le producteur diffuse header + hashes de tx. Les pairs reconstituent depuis leur mempool. Les txs manquantes sont demandées via TxRequest/TxResponse.
**Pourquoi** : Réduit la bande passante de propagation de blocs d'un facteur ~10 en charge normale.
**Implémenté dans** : `p2p/mod.rs`, `p2p/messages.rs`, `node.rs` (recent_block_txs cache), `chain.rs`
**Note** : Le cache `recent_block_txs` est indispensable car le producteur vide son mempool (update_confirmed_nonces) avant de diffuser — sans lui, TxRequest ne peut pas être servi.

---

### ADR-TECH-038 : Parallel sync au démarrage
Date : 26/08/2026 | Statut : **Implémenté**

**Contexte** : `sync_from_peer` télécharge les blocs un batch à la fois (séquentiel) — trop lent pour rattraper un réseau avec des milliers de blocs d'avance.
**Choix** : Pipeline en 3 étapes : (1) snapshot_sync si gap > 500 blocs, (2) parallel_sync avec 8 fetches HTTP concurrents, (3) sync_from_peer séquentiel pour la queue non finalisée.
**Pourquoi** : Réduit le temps de rattrapage de O(n) séquentiel à O(n/8) en téléchargement. Apply reste séquentiel pour garantir l'intégrité.
**Implémenté dans** : `sync.rs` (snapshot_sync_from_peer, parallel_sync_from_peer), `main.rs`

---

### ADR-TECH-SNAP : Chain.height_base (chaînes bootstrappées)
Date : 26/08/2026 | Statut : **Implémenté**

**Contexte** : Un nœud bootstrappé depuis un snapshot ne part pas de la hauteur 0 — l'arithmétique d'index de `Chain` cassait.
**Choix** : Champ `height_base: u64` avec serde default 0 (compat ascendante). Toute l'arithmétique d'index soustrait `height_base`. `Chain::new_from_snapshot(block)` initialise à la hauteur du snapshot.
**Pourquoi** : Solution minimale, zéro impact sur les chaînes existantes (height_base = 0).
**Implémenté dans** : `chain.rs`, `storage.rs`

---

### ADR 0064 : VinX = rail de paiement uniquement (abandon ZK/modules)
Date : 02/09/2026 | Statut : **Décidé** | Fichier : `docs/adr/0064-vinx-rail-paiement-uniquement.md`

**Contexte** : Choisir entre L1 généraliste (VM, ZK, modules, Appchains) ou rail de paiement pur.
**Choix** : Rail de paiement uniquement. Abandon du ZK (SP1/Groth16). ADRs 0001/0010/0024/0034/0048/0049/0050 gelés hors scope.
**Pourquoi** : Surface d'attaque réduite, auditabilité totale, message produit clair, pas de complexité ZK sans preuve de besoin, testnet d'abord.
**Implémenté dans** : Contrainte de périmètre — pas de code à ajouter. Docs à mettre à jour (whitepaper, README).

---

### ADR 0065 : Allocateur global mimalloc
Date : 02/09/2026 | Statut : **Décidé** | Fichier : `docs/adr/0065-allocateur-mimalloc.md`

**Contexte** : glibc malloc par défaut — fragmentation + contention sur workloads multi-threadés.
**Choix** : `mimalloc` via `#[global_allocator]` — 4 lignes, portable Linux/Mac/Windows.
**Pourquoi** : 10-20% de throughput gratuits, zéro impact protocole/wire format.
**Implémenté dans** : `Cargo.toml`, `crates/vinx-node/src/main.rs`

---

### ADR 0066 : Hash de transaction mémoïsé
Date : 02/09/2026 | Statut : **Décidé** | Fichier : `docs/adr/0066-hash-transaction-memoize.md`

**Contexte** : `transaction.hash()` recalculé à chaque point de contact (mempool, compact blocks, validation, sig cache).
**Choix** : Champ `#[serde(skip)] cached_hash: OnceLock<[u8;32]>` — calcul au premier appel, cache ensuite.
**Pourquoi** : Élimine N recalculs par tx par bloc. Zéro impact wire format.
**Implémenté dans** : `crates/vinx-core/src/transaction.rs`

---

### ADR 0067 : Cache de signatures mempool → validation de bloc
Date : 02/09/2026 | Statut : **Décidé** | Fichier : `docs/adr/0067-cache-signatures-mempool-bloc.md`

**Contexte** : La validation de bloc re-vérifie les signatures Ed25519 de txs déjà vérifiées à l'admission mempool.
**Choix** : `sig_cache: HashSet<[u8;32]>` dans le Mempool — skip de vérification si hash présent.
**Pourquoi** : 5-10x sur le chemin critique de validation de bloc en charge normale. Identique à Bitcoin Core.
**Implémenté dans** : `crates/vinx-node/src/mempool.rs`, handler de validation de bloc.

---

### ADR 0068 : Batch verify Ed25519
Date : 02/09/2026 | Statut : **Décidé** | Fichier : `docs/adr/0068-batch-verify-ed25519.md`

**Contexte** : Vérification individuelle ne profite pas de la structure algébrique de la courbe.
**Choix** : `ed25519_dalek::verify_batch()` sur les sigs non cachées. Fallback individuel si batch échoue.
**Pourquoi** : ~2x sur la vérification résiduelle. Zéro nouvelle dépendance (`ed25519-dalek` déjà présent).
**Implémenté dans** : `crates/vinx-node/src/p2p/mod.rs`

---

### ADR 0069 : BLAKE3 remplace SHA-256 ⚠️ avant genesis
Date : 02/09/2026 | Statut : **🔧 Implémenté (10/09/2026, avant genesis)** | Fichier : `docs/adr/0069-blake3-remplace-sha256.md`

**Contexte** : SHA-256 hérité — lent. BLAKE3 est 5-8x plus rapide sur matériel moderne (AVX2/NEON).
**Choix** : `vinx_crypto::sha256 → hash256` (crate `blake3`). Touche adresse, Merkle, block/tx hash, state_root, tiebreakers.
**Pourquoi** : Gain sur hot path. Changement breaking — impraticable après genesis, donc fait maintenant.
**Implémenté dans** : `crates/vinx-crypto/src/hash.rs` (+ address.rs, tout le workspace). `STORAGE_VERSION 19→20` — bases pré-BLAKE3 refusées (pas de migration possible). Test-garde `test_hash256_is_not_sha256`. Le ciphersuite BLS garde SHA-256 en interne (indépendant).
**⚠️ Contrainte** : fusionner AVANT le premier bloc du testnet. Après = hard fork.

---

## ⚠️ Risques actifs

> **3 maximum.** Résolus → déplacer en "Risques archivés".

| # | Risque | P | I | Score | Action |
|---|--------|---|---|-------|--------|
| R01 | Un seul validateur au genesis → liveness fault si le nœud tombe avant qu'un 2e rejoigne | H | H | HH | Lancer le 2e nœud dans les 24h suivant le genesis, documenter la procédure de rejoindre |
| R02 | RPC sans HTTPS → wallets refusent la connexion (mixed content) | H | M | HM | nginx + Let's Encrypt avant d'annoncer l'endpoint public |
| R03 | genesis_timestamp non coordonné → chaque nœud construit un genesis différent → sync cassée | M | H | MH | Fixer et annoncer la valeur avant que quiconque lance son nœud |

### Risques archivés
*(vide pour l'instant)*

---

## 📅 Jalons

| Jalon | Date cible | Statut |
|-------|-----------|--------|
| J0 — Protocole core complet (consensus, P2P, sync, storage) | Août 2026 | ✅ |
| J1 — Genesis testnet lancé, 1er bloc produit | Septembre 2026 | ⏳ |
| J2 — 3+ validateurs indépendants, monitoring actif | Octobre 2026 | ⏳ |
| J3 — Wallet public + faucet + doc opérateur | Q4 2026 | ⏳ |
| J4 — 30 jours sans interruption, 1ère intégration externe | Q1 2027 | ⏳ |
| J5 — Mainnet | 2027 | ⏳ |

---

## 📐 Règles de la méthode

1. **Ce fichier > tout autre doc** — en cas de contradiction entre PROJET_BRAIN.md et un autre fichier, ce fichier a raison et l'autre se met à jour.
2. **Mettre à jour "Où j'en suis" à chaque fin de session** — 3 lignes suffisent. Si c'est trop long, c'est que la session était trop longue.
3. **Focus = 3 items max** — si un 4e arrive, il remplace un des 3 ou va dans le backlog.
4. **Un ADR par décision** — même petites. "On n'a pas fait X" est aussi une décision valide.
5. **Risques = 3 max actifs** — au-delà c'est du bruit. Les résoudre avant d'en ajouter.
6. **Code et doc bougent ensemble** — si un PR change une décision technique, le Brain se met à jour dans le même commit.

---

*VinX Method v1.0 · haitodann@gmail.com*
