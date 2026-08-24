# ADR 0059 — API RPC REST

- **Statut :** Vérifié — ✅ implémenté avant la formalisation des ADRs
- **Date :** 2025 (pré-ADR) · Formalisé août 2026
- **Portée :** Interface — API HTTP JSON pour les wallets, explorateurs et opérateurs.
- **Décideur :** VinX Labs.
- **Crate :** `crates/vinx-node` — `src/rpc/`

---

## 1. Contexte

Les nœuds VinX doivent exposer une interface pour :
- Les wallets (soumettre des tx, consulter des soldes).
- Les explorateurs de blocs (lire l'état, lister les blocs).
- Les opérateurs (gérer les validateurs via les routes admin).
- Le faucet (devnet uniquement — distribuer des VINX de test).

## 2. Décision — API REST JSON sur Axum

**Framework :** Axum (tokio + tower) — async, performant, intégré à l'écosystème tokio déjà
utilisé par le nœud.

**Port par défaut :** `8080` (configurable dans `config.toml`).

### 2.1 Routes publiques

| Méthode | Chemin | Description |
|---|---|---|
| `GET` | `/health` | État du nœud, `finalized_height`, version |
| `GET` | `/status` | État complet (height, peers, mempool size, admin_address) |
| `GET` | `/balance/:address` | Solde VINX d'une adresse (balance + staked) |
| `GET` | `/account/:address` | Compte complet (balance, staked, nonce, unbonding) |
| `GET` | `/block/:height` | Bloc par hauteur |
| `GET` | `/block/latest` | Dernier bloc scellé |
| `GET` | `/tx/:hash` | Transaction par hash |
| `POST` | `/tx` | Soumet une transaction (JSON sérialisé) |
| `GET` | `/validators` | Set de validateurs actifs |
| `GET` | `/mempool` | Transactions en attente |
| `GET` | `/modules` | Registre des modules bondés |
| `GET` | `/proof/balance/:address` | Preuve Merkle du solde (pour les modules) |

### 2.2 Routes admin (fail-closed)

Protégées par `admin_token` dans le header `Authorization: Bearer <token>`.
**Fail-closed** : sans `admin_token` configuré, toutes les routes admin refusent
(`503 Service Unavailable` avec message explicite).

| Méthode | Chemin | Description |
|---|---|---|
| `POST` | `/admin/add-validator` | Propose l'ajout d'un validateur |
| `POST` | `/admin/remove-validator` | Propose la suppression d'un validateur |
| `POST` | `/admin/slash` | Soumet une preuve de slash |
| `GET` | `/admin/pending-governance` | Liste les propositions de gouvernance en attente |

### 2.3 Routes faucet (devnet uniquement)

Activées uniquement si `faucet_keypair` est configuré dans `config.toml`.

| Méthode | Chemin | Description |
|---|---|---|
| `POST` | `/faucet` | Envoie `faucet_amount_atoms` VINX à une adresse (cooldown configurable) |

### 2.4 Explorateur de blocs

Route `GET /` → UI HTML statique embarquée (ADR 0060).

## 3. Rate-limiting (ADR 0022)

Chaque IP est soumise à un token bucket : `max_burst` requêtes en rafale, `refill_rate`
requêtes/seconde. Les routes admin et faucet ont des limites plus strictes.
Voir `src/rpc/rate_limit.rs`.

## 4. Critères de validation

- [x] `GET /health` retourne `200` avec `finalized_height` quand le nœud est synchronisé.
- [x] `POST /tx` accepte une tx valide et la propage au mempool + P2P.
- [x] Les routes admin refusent avec `401` si `admin_token` est incorrect.
- [x] Les routes admin refusent avec `503` si aucun `admin_token` n'est configuré.
- [x] Le faucet respecte le cooldown par adresse.
- [x] `cargo test --workspace` vert (tests d'intégration dans `vinx-node/tests/`).

## 5. Conséquences

- **Positif :** API JSON simple, compatible avec n'importe quel client HTTP.
- **Positif :** routes admin fail-closed → un opérateur qui oublie de configurer le token
  n'est pas silencieusement vulnérable.
- **Compromis :** REST (pas de WebSocket) → pas de streaming de nouveaux blocs ; les clients
  doivent polluer (longpolling ou intervalle fixe).
- **Compromis :** pas de versionnage d'API explicite (`/v1/`) — à ajouter si l'API doit
  évoluer sans casser les clients existants.
