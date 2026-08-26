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
[26/08/2026 — ~4h]
✅ Fait :     ADR 0037 côté producteur (compact blocks + cache recent_block_txs)
              Snapshot sync (GET /chain/snapshot + snapshot_sync_from_peer)
              Parallel sync ADR 0038 (8 fetches concurrents, apply séquentiel)
              Chain height_base pour chaînes bootstrappées depuis snapshot
              Dockerfile multi-stage + docker-entrypoint.sh + docker-compose.testnet.yml
              genesis-testnet.json + config.testnet.toml (templates)
              Décision produit : VinX = rail de paiement uniquement (pas de VM)

🔜 Next :     Remplir genesis-testnet.json avec les vraies adresses (admin + validateur)
              Lancer le nœud de genèse sur un serveur public
              Reverse proxy HTTPS (nginx + Let's Encrypt) sur le port 8545

🤖 AI :       Demandé → "Fais moi les 2" (compact blocks producteur + snapshot sync)
              Codé → ADR 0037 full, ADR 0038, Chain.height_base, Dockerfile, genesis template
              ⚠️ Divergence → —

🚧 Bloqué :   Besoin des adresses admin + validateur réelles pour finaliser genesis-testnet.json
```

---

## ⚡ Focus maintenant

> **3 items maximum.** Tout le reste attend dans le Backlog.

- [ ] Remplir `genesis-testnet.json` (admin_address, initial_validator, genesis_timestamp fixé)
- [ ] Déployer le nœud de genèse sur serveur public + HTTPS
- [ ] Vérifier que 3 nœuds se synchronisent depuis le même genesis hash

---

## 📋 Backlog vivant

### Up next *(les 5 suivantes, ordonnées par priorité)*
- Monitoring : Prometheus + Grafana sur `/metrics` — alerte si bloc stall > 2× block_time
- Faucet public accessible depuis l'interface web (déjà codé, à exposer)
- Documentation opérateur : "Rejoindre le testnet en 10 minutes" (README ou GETTING_STARTED.md)
- Script de vérification genesis : compare le hash du bloc 0 entre pairs et échoue si divergence
- Wallet web/mobile minimaliste pour le testnet (balance, send, historique)

### Someday / Maybe *(idées capturées sans engagement)*
- Bridge Ethereum → VinX (wrapped VinX sur Ethereum, rédemption sur VinX)
- Modules de transaction supplémentaires (si besoin réel identifié post-testnet, pas avant)
- Explorer blockchain public hébergé (aujourd'hui disponible via `/block/:height` + `/metrics`)
- SDK JavaScript pour intégrations tierces
- Programme de validateurs testnet (incentives, documentation, onboarding)

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
