# ADR 0053 — Anti-replay : nonce, chain_id, expiry_height

- **Statut :** Vérifié — ✅ implémenté avant la formalisation des ADRs
- **Date :** 2025 (pré-ADR) · Formalisé août 2026
- **Portée :** Sécurité — protection contre la double-dépense et le rejeu de transactions.
- **Décideur :** VinX Labs.
- **Crate :** `crates/vinx-core` — `src/transaction.rs`
- **Révisé par :** [ADR 0073](./0073-injectivite-serialisation-signee.md) (§3 ci-dessous : layout des `signing_bytes`)

---

## 1. Contexte

Sans protection anti-replay, une transaction signée valide peut être ré-émise indéfiniment
(par un attaquant qui intercepte une tx) ou sur un autre réseau (ex. rejouer une tx de
devnet sur mainnet). VinX doit immuniser les transactions contre ces attaques.

## 2. Décision — 3 couches d'anti-replay

### 2.1 Nonce (séquence par compte)

Chaque transaction inclut un `nonce: u64` incrémental par adresse expéditeur. Le WorldState
maintient `account.nonce` — le dernier nonce accepté. Toute tx dont le nonce n'est pas
`account.nonce + 1` est rejetée.

- **Règle du mempool :** les tx à nonce futur (nonce > nonce_actuel + 1) sont acceptées dans
  le mempool mais pas appliquées — elles attendent les tx intermédiaires.
- **Garantie :** une tx ne peut être appliquée qu'une seule fois, dans l'ordre exact.

### 2.2 Chain ID (isolation de réseau)

Chaque transaction inclut `chain_id: u32`. La règle de validation rejette toute tx dont le
`chain_id` ne correspond pas au `chain_id` du nœud.

- Valeur par défaut sûre : voir ADR 0008 (pas de défaut silencieux).
- `CHAIN_ID_MAINNET = 1`, `CHAIN_ID_TESTNET = 7`, `CHAIN_ID_DEVNET = 42` (cf. `chain_id.rs` ; le devnet est le défaut des constructeurs de tx en local).

### 2.3 Expiry height (expiration automatique)

Chaque transaction peut inclure un `expiry_height: Option<u64>`. Si défini, la tx est
invalide si `current_height > expiry_height`.

- Protège contre le stockage indéfini de tx valides dans un mempool hors-ligne.
- Le mempool appelle `prune_expired(current_height)` à chaque bloc.

### 2.4 Sponsor (protection des comptes tiers)

Une tx peut être co-signée par un `sponsor: Option<PublicKey>` qui paie les frais à la place
de l'expéditeur. La signature du sponsor est vérifiée séparément → pas de rejeu possible.

> ⚠️ **Correction (septembre 2026, finding VINX-03).** Cette garantie était **décrite mais
> non implémentée**. Aucun des trois points d'entrée du mempool ne vérifiait la signature du
> sponsor : n'importe qui pouvait désigner un compte tiers comme `sponsor`, fixer `fee` à son
> solde entier et le **vider** sans son consentement. Les trois chemins appellent désormais
> l'unique prédicat canonique `WorldState::verify_tx_signature_pure`. Voir
> `audit/post-fix/PATCH.md`.

## 3. Intégration dans les signing_bytes

Les `signing_bytes` (message signé) incluent **tous les champs anti-replay**. Layout
**actuel et faisant foi** (voir ADR 0073) :

```
[tx_type(1o)] [from(20o)] [to(20o)] [amount(16o)] [fee(16o)] [nonce(8o)]
[chain_id(4o)] [expiry(0x00 | 0x01‖8o)] [payload_len(4o BE)] [payload(var)]
[sponsor(0x00 | 0x01‖20o)]
```

Ainsi la signature couvre `chain_id`, `nonce` et `expiry_height` → impossible de modifier
ces champs après signature.

> ⚠️ **Correction (septembre 2026, finding VINX-12).** Le layout décrit ici à l'origine
> annonçait un `payload_len(8o)` que le code **n'a jamais implémenté**, et omettait le
> marqueur de sponsor. `payload` était concaténé brut, immédiatement suivi du marqueur : pour
> toute adresse de sponsor finissant par `0x00`, deux transactions économiquement
> différentes produisaient des octets signés et un **txid identiques**. Corrigé par un
> préfixe de longueur `u32` BE écrit inconditionnellement (ADR 0073).
>
> **Leçon :** un layout consensus-critique décrit en prose dérive du code sans que personne
> ne s'en aperçoive. Il doit être figé par un **vecteur doré** (ADR 0020) — ce que font
> désormais `test_signing_bytes_golden_vector` et
> `test_governance_signing_bytes_golden_vector`.

## 4. Critères de validation

- [x] Tx avec nonce dupliqué rejetée (`NonceAlreadyUsed`).
- [x] Tx avec mauvais `chain_id` rejetée.
- [x] Tx avec `expiry_height` dépassé purgée du mempool et rejetée en bloc.
- [x] Tx avec nonce futur acceptée dans le mempool mais pas appliquée immédiatement.
- [x] `cargo test --workspace` vert (vecteurs de test sur `signing_bytes`).
