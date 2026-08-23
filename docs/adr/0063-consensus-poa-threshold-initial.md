# ADR 0063 — Consensus PoA Threshold (implémentation initiale)

- **Statut :** Implémenté — production actuelle ; sera remplacé par ADR 0029 (PoS Algorand-style)
- **Date :** 2025 (pré-ADR) · Formalisé août 2026
- **Portée :** Consensus — mécanisme de production et de finalisation des blocs actuellement en production.
- **Décideur :** VinX Labs.
- **Crates :** `crates/vinx-node` — `src/consensus.rs`, `src/producer.rs`, `src/chain.rs`

---

## 1. Contexte

VinX a été développé avec un consensus initial simple pour permettre de valider
rapidement les autres briques (émission, gouvernance, state machine, P2P). Ce consensus
est volontairement plus simple que la cible PoS Algorand-style (ADR 0029) — il ne requiert
pas de VRF ni d'agrégation BLS pour fonctionner.

Ce ADR documente l'implémentation actuelle **telle qu'elle existe en production**, pas la
cible architecturale.

## 2. Mécanisme — PoA Threshold (round-robin + quorum BFT)

### 2.1 Sélection du leader (round-robin)

Le leader du bloc à la hauteur `h` est déterminé par :
```
leader_index = h % active_set.len()
active_set   = validators triés par adresse (déterministe)
```

**Avantage :** simple, déterministe, aucune aléatoire nécessaire.
**Limite :** le leader est prédictible → vecteur de DoS ciblé (adressé par ADR 0029 via VRF).

Les validateurs jailés (ADR 0027) sont **sautés** dans la rotation (set actif ≠ set complet).

### 2.2 Quorum BFT

- Quorum de finalité : **⌈2n/3⌉ co-signatures** sur le set **complet** bondé (pas le set actif).
- À n=1 : finalité immédiate (le proposeur seul = quorum).
- À n=3 : 2/3 nœuds nécessaires.
- Le jailing ne réduit **jamais** le quorum (ADR 0002 — règle BFT de sûreté).

### 2.3 Production d'un bloc

1. Le leader `produce_block()` :
   - Extrait ≤ `MAX_BLOCK_TXS = 3000` tx du mempool.
   - Calcule l'émission de l'époque si applicable.
   - Applique les tx au `WorldState` (via `settle_block`).
   - Calcule le `state_root` (Merkle du WorldState).
   - Scelle l'en-tête et le gossipe.

2. Les non-leaders reçoivent le bloc via P2P :
   - Vérifient l'en-tête (signature du proposeur, state_root).
   - Envoient leur co-signature BLS ou Ed25519.

3. Quand ⌈2n/3⌉ co-signatures reçues : `finalized_height` avance (ADR 0002).

### 2.4 Backup (anti-liveness)

Si le leader prévu ne produit pas de bloc dans `BLOCK_TIME_SECS = 12` s, le **backup**
(prochain validateur dans la rotation) prend le relais. Limite : `MAX_UNFINALIZED_DEPTH = 64`.

## 3. Paramètres actifs

| Paramètre | Valeur | Source |
|---|---|---|
| `BLOCK_TIME_SECS` | 12 | ADR 0043 |
| `MAX_BLOCK_TXS` | 3 000 | ADR 0043 |
| `MAX_UNFINALIZED_DEPTH` | 64 | ADR 0002 |
| `N_ACTIVE_DEFAULT` | 21 | PROTOCOL_SPEC.md |

## 4. Critères de validation

- [x] À n=1 : production continue de blocs à ~12 s d'intervalle.
- [x] À n=3 : finalité après 2 co-signatures, banc validé.
- [x] Rotation leader correcte (n=3 : 3 leaders distincts sur 100 blocs).
- [x] Backup déclenché si le leader est absent (simulé par kill -STOP).
- [x] `cargo test --workspace` vert.

## 5. Plan de remplacement — ADR 0029

Ce mécanisme sera remplacé par le **comité VRF Algorand-style** (ADR 0029) :
- Leader = validateur avec la sortie ECVRF la plus faible (imprévisible).
- Co-signatures BLS12-381 agrégées (n signatures → 1 agrégat).
- Comité de taille `COMMITTEE_SIZE_TARGET = 100` sélectionné parmi le pool bondé.

Le remplacement est un changement consensus-breaking → incrémentera `STORAGE_VERSION`.
