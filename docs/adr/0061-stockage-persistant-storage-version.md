# ADR 0061 — Stockage persistant & STORAGE_VERSION

- **Statut :** Vérifié — ✅ implémenté avant la formalisation des ADRs
- **Date :** 2025 (pré-ADR) · Formalisé août 2026
- **Portée :** Infrastructure — persistance de la chaîne et du WorldState entre redémarrages.
- **Décideur :** VinX Labs.
- **Crate :** `crates/vinx-node` — `src/storage.rs`
- **STORAGE_VERSION actuelle :** **20** (backend **redb**, fichier `vinx.redb`)

> ⚠️ **Mises à jour depuis la rédaction :**
> - **Backend = `redb`** (et non sled/RocksDB) : base KV transactionnelle en Rust pur, fichier `vinx.redb`.
> - **`STORAGE_VERSION = 20`** (la migration automatique décrite en §2.2 reste vraie pour les
>   sauts intra-protocole ; **exception BLAKE3** : depuis v20, une base pré-BLAKE3 (≤ v19) est
>   **refusée** avec un message actionnable et laissée intacte — aucune migration possible, les
>   empreintes stockées appartiennent à un autre protocole de hachage, ADR 0069).

---

## 1. Contexte

Le nœud VinX doit survivre aux redémarrages. L'état (`WorldState`) et la chaîne (`Chain`)
doivent être persistés sur le disque et rechargés fidèlement. Le schéma de persistance évolue
avec chaque ADR consensus-breaking → un mécanisme de versionnage et de migration est
indispensable.

## 2. Décision

### 2.1 Base de données — redb

**Choix retenu (implémenté) :** `redb` — base KV transactionnelle en Rust pur, embarquée, une
seule dépendance, pas de portage C++. Les écritures passent par des transactions ACID (une
migration partielle ne peut donc pas corrompre la base). Fichier unique `vinx.redb` dans le
répertoire de données.

### 2.2 STORAGE_VERSION

```rust
const STORAGE_VERSION: u64 = 20;
```

À chaque démarrage du nœud :
- Si la version sur disque == `STORAGE_VERSION` : chargement normal.
- Si la version sur disque < `STORAGE_VERSION` : **migration automatique** (séquence de
  migrations `from v → v+1`, appliquées jusqu'à atteindre la version cible).
- Si la version sur disque > `STORAGE_VERSION` : **erreur fatale** (binaire trop vieux,
  refus de démarrer plutôt que de corrompre des données au format plus récent).

### 2.3 Sérialisation — bincode

Le `WorldState` et la `Chain` sont sérialisés avec **bincode** (format binaire compact,
déterministe, Rust-natif). Aucun JSON ni protobuf pour les données internes (performance
et taille compacte).

**Règle :** `STORAGE_VERSION` doit être incrémentée à chaque changement de schéma
(ajout/suppression de champ dans `WorldState`, nouveau type de tx, etc.).

### 2.4 Stratégies de sauvegarde

```rust
StateWrite::serialize_incremental(state, chain)  // Sérialise uniquement le delta depuis le snapshot
StateWrite::serialize_full(state, chain)          // Snapshot complet (périodique)
Storage::save_mempool_blob(blob)                  // Persiste le mempool pour récupération après crash
Storage::load_mempool()                           // Recharge le mempool au démarrage
```

**Sauvegarde incrémentale :** à chaque bloc, seulement le delta (plus rapide).
**Snapshot complet :** périodiquement (ex. toutes les N époques) → point de départ pour
les réorganisations (ADR 0031) et le fast-sync.

### 2.5 Historique des migrations (STORAGE_VERSION)

| Version | Changement |
|---|---|
| 1–9 | Évolutions initiales (pré-ADR) |
| 10 | ADR 0040 — émission progressive (champ `emitted_atoms`, suppression `foundry`) |
| 11 | ADR 0027 — fiabilité validateurs (champ `reliability: BTreeMap`) |
| 12 | ADR 0031 — fork-choice (snapshot finalisé) |
| 13 | ADR 0046 — BLS (champ `bls_keys: BTreeMap`) |
| 14 | ADR 0010 — modules (champ `modules: BTreeMap`) |

## 3. Critères de validation

- [x] Démarrage sur schéma v13 → migration automatique vers v14 (testé).
- [x] Démarrage sur schéma v15 (futur) → erreur fatale claire (testé).
- [x] Crash au milieu d'une écriture → rechargement cohérent (pas de corruption).
- [x] `cargo test --workspace` vert (tests dans `vinx-node/tests/storage_test.rs`).

## 4. Conséquences

- **Positif :** migrations automatiques → les opérateurs n'ont pas à intervenir manuellement.
- **Positif :** `STORAGE_VERSION` force une discipline de versionnage (chaque ADR
  consensus-breaking incrémente la version).
- **Compromis :** bincode n'est pas auto-descriptif — si un champ est mal ordonné ou
  manquant, la migration peut corrompre des données silencieusement (risque mitigé par
  les tests de migration).
- **Compromis :** redb reste jeune face à RocksDB pour de très gros volumes — à réévaluer à
  l'échelle mainnet ; en contrepartie, zéro dépendance C++ et des transactions ACID natives.
