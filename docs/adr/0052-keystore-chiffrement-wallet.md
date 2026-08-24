# ADR 0052 — Keystore — chiffrement du fichier portefeuille

- **Statut :** Vérifié — ✅ implémenté avant la formalisation des ADRs
- **Date :** 2025 (pré-ADR) · Formalisé août 2026
- **Portée :** Sécurité — protection de la clé privée Ed25519 sur le disque.
- **Décideur :** VinX Labs.
- **Crate :** `crates/vinx-wallet` — `src/keystore.rs`

---

## 1. Contexte

La clé privée Ed25519 d'un validateur ou d'un utilisateur doit être stockée sur le disque
de façon sécurisée. Si le fichier wallet est volé (backup, fuite de VM), la clé ne doit
pas être récupérable sans la passphrase.

## 2. Décision

**Format keystore : JSON chiffré AES-256-GCM avec dérivation Argon2id.**

```json
{
  "crypto_params": {
    "algorithm": "argon2id-aes256gcm",
    "salt": "<hex 32 o>",
    "nonce": "<hex 12 o>",
    "m_cost": 65536,
    "t_cost": 3,
    "p_cost": 1
  },
  "ciphertext": "<hex — clé privée 32 o chiffrée>",
  "address": "vinx1..."
}
```

**Pourquoi AES-256-GCM ?**
- Authentification intégrée (AEAD) → toute altération du ciphertext est détectée.
- Standard, hardware-acceleré sur x86 (AES-NI).

**Pourquoi Argon2id ?**
- Dérivation mémoire-dure → résistant aux attaques GPU/ASIC sur la passphrase.
- Paramètres : 64 MiB RAM, 3 passes, 1 thread (raisonnable sur un laptop, coûteux à bruteforcer).
- Vainqueur du Password Hashing Competition 2015 ; recommandé par OWASP.

**Mode sans passphrase :** si l'opérateur ne fournit pas de passphrase (nœud automatisé),
la clé privée est stockée **en clair dans le JSON** (le champ `ciphertext` est la clé brute).
Ce mode est documenté comme non recommandé pour les environnements de production.

## 3. API

```rust
KeyStore::generate()              // Génère une nouvelle paire, retourne (KeyStore, KeyPair)
KeyStore::from_keypair(kp)        // Crée un KeyStore depuis une paire existante
KeyStore::save(path, passphrase)  // Chiffre et sauvegarde sur le disque
KeyStore::load(path)              // Charge le JSON (ne déchiffre pas encore)
KeyStore::to_keypair()            // Déchiffre et retourne la KeyPair
KeyStore::address()               // Retourne l'adresse Bech32 (stockée en clair dans le JSON)
```

## 4. Critères de validation

- [x] Round-trip : `save` + `load` + `to_keypair` retourne la même paire.
- [x] Avec passphrase incorrecte : `to_keypair` retourne une erreur, pas de panic.
- [x] Sans passphrase : le fichier est marqué comme non-chiffré.
- [x] Altération du ciphertext (1 bit) : `to_keypair` retourne une erreur (AEAD).
- [x] `cargo test --workspace` vert.

## 5. Conséquences

- **Positif :** la clé privée n'est jamais en clair dans la mémoire après déchiffrement
  plus longtemps que nécessaire (pas de persistance supplémentaire).
- **Compromis :** le mode sans passphrase est dangereux — documenté mais non bloqué.
- **Compromis :** Argon2id ajoute ~1 s de délai au démarrage (acceptable, une seule fois).
