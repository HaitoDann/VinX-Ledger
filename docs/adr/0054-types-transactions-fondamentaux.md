# ADR 0054 — Types de transactions fondamentaux (0x01–0x0A)

- **Statut :** Vérifié — ✅ implémenté avant la formalisation des ADRs
- **Date :** 2025 (pré-ADR) · Formalisé août 2026
- **Portée :** Protocole — catalogue des types de transactions L1 et leurs règles de validation.
- **Décideur :** VinX Labs.
- **Crate :** `crates/vinx-core` — `src/transaction.rs`

---

## 1. Contexte

VinX est une monnaie pure : les transactions représentent les seules mutations de l'état L1.
Il faut définir un catalogue fermé de types de transactions, avec des règles de validation
strictes pour chacun. L'espace de types est un octet (0x00–0xFF) — les types non définis
sont rejetés.

## 2. Catalogue des types (au 23 août 2026)

| Type | Octet | Nom | Description | Frais |
|---|---|---|---|---|
| `Transfer` | 0x01 | Transfert VINX | Envoie N atoms de `from` à `to` | Forfait plat (ADR 0055) |
| `Stake` | 0x02 | Mise en gage | Dépose N atoms dans le bond validateur | Exempt (ADR 0009) |
| `Unstake` | 0x03 | Retrait de gage | Déclenche une déliaison (unbonding, ADR 0056) | Exempt (ADR 0009) |
| `AnnounceUpgrade` | 0x04 | Annonce d'upgrade | Signale une version de protocole à activer à une date (ADR 0006) | Forfait plat |
| `SlashValidator` | 0x07 | Slashing | Soumet une `SlashEvidence` pour punir un équivocateur (ADR 0003) | Zéro (incentivé par la récompense) |
| `AdminAction` | 0x08 | Action de gouvernance | Porte un `GovernanceAction` bincode-sérialisé (ADR 0007) | Forfait plat |
| `AnchorState` | 0x09 | Ancrage de module | Porte un `ModuleOp` (Register/Anchor/Deregister) (ADR 0010) | Forfait plat |
| `RegisterBlsKey` | 0x0A | Enregistrement BLS | Associe une clé BLS12-381 à un validateur (ADR 0046) | Forfait plat |

**Discriminants 0x05/0x06 retirés** (ADR 0007 — `AddValidator`/`RemoveValidator` unifiés dans `AdminAction`).

**Types à venir :**
- `ForceExit` (0x0B) — ADR 0048
- `CrossMsgExpire` (0x0C) — ADR 0049

## 3. Structure commune d'une transaction

```rust
Transaction {
    tx_type:      TransactionType,    // 1 octet
    from:         Address,            // 20 octets
    to:           Address,            // 20 octets
    amount:       Amount,             // 16 octets (u128 atoms)
    fee:          Amount,             // 16 octets
    nonce:        u64,                // 8 octets
    chain_id:     u32,                // 4 octets
    expiry_height: Option<u64>,       // 9 octets (tag + valeur)
    payload:      Vec<u8>,            // variable
    signature:    VinxSignature,      // 64 octets
    sponsor:      Option<SponsorSig>, // optionnel
}
```

## 4. Règles de validation communes

1. `from` dérivable de la signature (Ed25519 verify sur `signing_bytes`).
2. `chain_id` correspond au réseau (ADR 0053).
3. `nonce == account.nonce + 1` (ADR 0053).
4. `expiry_height` non dépassé (ADR 0053).
5. `fee >= BASE_FEE` (ADR 0055) — sauf exemptions documentées.
6. `from` solde >= `amount + fee` (pour Transfer).

## 5. Critères de validation

- [x] Chaque type de tx a un vecteur de test canonique (signing_bytes figés).
- [x] Les discriminants supprimés (0x05/0x06) sont rejetés avec `UnknownTxType`.
- [x] Types inconnus rejetés à la désérialisation.
- [x] `cargo test --workspace` vert.

## 6. Conséquences

- **Positif :** catalogue fermé → surface d'attaque bornée, auditabilité simple.
- **Compromis :** tout nouveau type de tx est un changement consensus-breaking — nécessite
  un ADR dédié et une migration de `STORAGE_VERSION`.
