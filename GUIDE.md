# Guide de démarrage VinX Ledger

## Prérequis

- **Rust** ≥ 1.75 — installer via [rustup.rs](https://rustup.rs)
- **Git**
- Un terminal (Linux / macOS / WSL / PowerShell / cmd.exe)

Vérifier l'installation :
```bash
rustc --version   # rustc 1.75.0 ou supérieur
cargo --version
```

---

## Note importante — le séparateur `--`

Toutes les commandes `cargo run -p vinx-wallet` **exigent** un `--` entre les
arguments de cargo et ceux du wallet.

```
cargo run -p vinx-wallet -- <sous-commande> [options]
                          ^^
                   obligatoire
```

Sans le `--`, cargo intercepte les flags (`-output`, `--amount`, etc.) et
affiche une erreur du type `unexpected argument 'my-wallet.json' found`.

---

## Syntaxe multi-lignes selon le terminal

Les exemples ci-dessous proposent des variantes pour chaque système.

| Terminal | Continuation de ligne |
|---|---|
| Linux / macOS / WSL (bash/zsh) | `\` en fin de ligne |
| PowerShell (Windows) | `` ` `` (backtick) en fin de ligne |
| cmd.exe (Windows) | `^` en fin de ligne |

---

## 1. Cloner le projet

```bash
git clone https://github.com/HaitoDann/VinX-Ledger.git
cd VinX-Ledger
```

---

## 2. Lancer les tests

Vérifier que tout compile et passe avant de démarrer :

```bash
cargo test --workspace
```

Résultat attendu : **118 tests, 0 failures**.

---

## 3. Démarrer le nœud (devnet)

```bash
cargo run -p vinx-node
```

Au premier démarrage, le nœud génère les identités et affiche :

```
════════════════════════════════════════════════════════
  VinX Ledger — DEVNET  (block time: 3s | RPC: :8545)
  Nouveau genesis
════════════════════════════════════════════════════════
  Admin     : vinx1xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
             21,000,000.00 VINX — clé dans devnet/admin.json
  Validator : vinx1yyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyy
════════════════════════════════════════════════════════
```

Les clés sont sauvegardées dans `devnet/` et rechargées à chaque redémarrage.
**Le nœud produit un bloc toutes les 3 secondes et sauvegarde l'état sur disque (`devnet/state.bin`, `devnet/chain.bin`).**

Aux redémarrages suivants, le nœud reprend là où il s'est arrêté :

```
════════════════════════════════════════════════════════
  VinX Ledger — DEVNET  (block time: 3s | RPC: :8545)
  Reprise depuis le bloc 42
════════════════════════════════════════════════════════
```

> Laisser ce terminal ouvert. Ouvrir un second terminal pour la suite.

---

## 4. Créer son wallet

```bash
cargo run -p vinx-wallet -- keygen --output my-wallet.json
```

Sortie :
```
Address : vinx1aabbccddee...
Saved   : my-wallet.json
```

Afficher son adresse à tout moment :
```bash
cargo run -p vinx-wallet -- address --wallet my-wallet.json
```

---

## 5. Recevoir des VINX depuis l'admin

L'admin dispose de **21 000 000 VINX** au genesis (Sandbox Phase 1). Sa clé est dans `devnet/admin.json`.

**Linux / macOS / WSL :**
```bash
cargo run -p vinx-wallet -- transfer \
  --wallet devnet/admin.json \
  --to <VOTRE_ADRESSE> \
  --amount 10000
```

**PowerShell :**
```powershell
cargo run -p vinx-wallet -- transfer `
  --wallet devnet/admin.json `
  --to <VOTRE_ADRESSE> `
  --amount 10000
```

**cmd.exe :**
```cmd
cargo run -p vinx-wallet -- transfer ^
  --wallet devnet/admin.json ^
  --to <VOTRE_ADRESSE> ^
  --amount 10000
```

**En une seule ligne (universel) :**
```bash
cargo run -p vinx-wallet -- transfer --wallet devnet/admin.json --to <VOTRE_ADRESSE> --amount 10000
```

Sortie :
```
From    : vinx1xxxxxxx... (admin)
To      : vinx1aabbcc... (vous)
Amount  : 10000.00 VINX
Fee     : 5.00 VINX
Nonce   : 0
Status  : accepted
Tx hash : 3f8a1b2c...
```

La transaction est dans le **mempool**. Elle sera incluse dans le prochain bloc (~3s).

---

## 6. Vérifier son solde

```bash
cargo run -p vinx-wallet -- balance <VOTRE_ADRESSE>
```

Sortie (après le prochain bloc) :
```
Address : vinx1aabbcc...
Balance : 10000.00 VINX
Staked  : 0.00 VINX
Nonce   : 0
Frozen  : no
```

> Si le solde est encore à 0, attendre quelques secondes le prochain bloc.

---

## 7. Envoyer des VINX à une autre adresse

Créer un second wallet :
```bash
cargo run -p vinx-wallet -- keygen --output alice.json
cargo run -p vinx-wallet -- address --wallet alice.json
# → vinx1alice...
```

Envoyer depuis son propre wallet (**une seule ligne**) :
```bash
cargo run -p vinx-wallet -- transfer --wallet my-wallet.json --to <ADRESSE_ALICE> --amount 500
```

---

## 8. Staker des VINX

Le staking accumule 80% des frais de toutes les transactions du réseau.

```bash
# Staker 1000 VINX
cargo run -p vinx-wallet -- stake --wallet my-wallet.json --amount 1000

# Vérifier : la colonne "Staked" augmente
cargo run -p vinx-wallet -- balance <VOTRE_ADRESSE>

# Récupérer ses tokens
cargo run -p vinx-wallet -- unstake --wallet my-wallet.json --amount 1000
```

---

## 9. Explorer la chaîne

**Statut du nœud :**
```bash
cargo run -p vinx-wallet -- status
```
```
Node    : http://127.0.0.1:8545
Status  : ok
Height  : 12
Mempool : 0 pending
```

**Détails d'un bloc :**
```bash
cargo run -p vinx-wallet -- block 1
```
```
Height    : 1
Hash      : 4e9f2a...
Prev hash : 000000...
Timestamp : 1748736003
Validator : vinx1yyy...
Tx count  : 1
Transactions:
  Emission  abc123...  vinx1yyy...  →  vinx1zzz...  fee 0.00 VINX
```

**Via curl (API brute) :**
```bash
curl http://localhost:8545/health
curl http://localhost:8545/chain/height
curl http://localhost:8545/account/<ADRESSE>
curl http://localhost:8545/block/0
curl http://localhost:8545/mempool/size
```

---

## 10. Référence des commandes

### `vinx-wallet`

| Commande | Description |
|---|---|
| `keygen --output <fichier>` | Génère une paire de clés Ed25519 |
| `address --wallet <fichier>` | Affiche l'adresse du wallet |
| `balance <adresse>` | Solde, staked, nonce, frozen |
| `transfer --wallet <f> --to <addr> --amount <n>` | Envoyer des VINX |
| `stake --wallet <f> --amount <n>` | Staker des VINX |
| `unstake --wallet <f> --amount <n>` | Récupérer des VINX stakés |
| `block <hauteur>` | Détails d'un bloc |
| `status` | État du nœud (hauteur, mempool) |

Toutes les commandes réseau acceptent `--node <url>` (défaut : `http://127.0.0.1:8545`).

### Endpoints RPC

| Méthode | Route | Description |
|---|---|---|
| `GET` | `/health` | Statut, hauteur, mempool |
| `GET` | `/chain/height` | Hauteur du dernier bloc |
| `GET` | `/account/:address` | Compte (balance, nonce, staked, frozen) |
| `POST` | `/tx/submit` | Soumettre une transaction JSON signée |
| `GET` | `/block/:height` | Bloc et ses transactions |
| `GET` | `/mempool/size` | Transactions en attente |

---

## 11. Structure du projet

```
VinX-Ledger/
├── crates/
│   ├── vinx-crypto/   Ed25519, adresses Bech32, SHA-256
│   ├── vinx-core/     Amount, Account, Transaction, Block
│   ├── vinx-state/    WorldState, genesis, apply_transaction
│   ├── vinx-node/     Nœud, mempool, RPC HTTP, producteur de blocs
│   └── vinx-wallet/   CLI wallet
├── devnet/            Clés générées au premier démarrage (gitignorées)
└── README.md          Spécification du protocole
```

---

## 12. Dépannage

**`unexpected argument '...' found` ou `error: Found argument '-output'`**
→ Le `--` entre `cargo run -p vinx-wallet` et la sous-commande est **obligatoire**.
  Correct : `cargo run -p vinx-wallet -- keygen --output my-wallet.json`
  Incorrect : `cargo run -p vinx-wallet keygen --output my-wallet.json`
  Incorrect : `cargo run -p vinx-wallet -output my-wallet.json`

**Le solde ne change pas après le transfer**
→ Attendre le prochain bloc (~3s). Le nœud doit être démarré.

**`Error: Node unreachable`**
→ Vérifier que `cargo run -p vinx-node` tourne dans un autre terminal.

**`Error: Account not found` en vérifiant le solde admin**
→ Utiliser l'adresse affichée dans le banner du nœud, pas celle du fichier `devnet/admin.json` avant le premier démarrage.

**Repartir de zéro**
→ Supprimer le dossier `devnet/`. Le nœud génère de nouvelles identités au prochain démarrage.

Linux/macOS/WSL :
```bash
rm -rf devnet/
```
Windows (PowerShell) :
```powershell
Remove-Item -Recurse -Force devnet\
```
Windows (cmd.exe) :
```cmd
rmdir /s /q devnet
```

**Lancer les tests d'intégration seuls**
```bash
cargo test -p vinx-node --test integration
```
