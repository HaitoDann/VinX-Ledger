# VinX Wallet — application desktop (Tauri)

Un portefeuille et une console d'administration **de bureau** pour VinX Ledger.
La clé privée est gérée par le backend Rust **en local** : elle ne quitte jamais
l'application et n'est jamais envoyée au nœud. L'app parle au nœud VinX en RPC HTTP.

```
apps/vinx-desktop/
├── src-tauri/     backend Rust (commandes Tauri, appels RPC) — workspace séparé
│   ├── src/main.rs
│   ├── tauri.conf.json · capabilities/ · icons/ · build.rs
│   └── Cargo.toml
├── ui/            frontend statique (HTML/CSS/JS, aucune étape de build)
└── app-icon.png   source pour générer les icônes multi-plateformes
```

La logique métier (keystore, parsing de montants, construction/signature des
transactions) vit dans le crate `crates/vinx-desktop-core` — testé par la CI du
workspace principal. Seule la coquille Tauri (réseau + UI) est ici.

## Fonctionnalités

**Portefeuille** — ouvrir/créer un wallet (format compatible avec `vinx-wallet` CLI),
adresse + copie (pour recevoir), solde/bond/nonce, envoi de VINX, stake/déstake,
historique, et l'état du réseau (hauteur, mempool, frais de base, circulation, émission progressive).

**Admin** — vérifie que le wallet ouvert est bien l'admin on-chain, liste les
validateurs (leader / en ligne / suspendu), ajout/retrait de validateur, et
planification d'une mise à jour de protocole.

## Prérequis

- **Rust** (stable) → https://rustup.rs
- **Tauri CLI v2** : `cargo install tauri-cli --version "^2.0"`
- **Dépendances système** (Linux : `webkit2gtk-4.1`, `libayatana-appindicator`,
  etc.) — voir https://tauri.app/start/prerequisites/ pour votre OS.
- **Un nœud VinX qui tourne** (depuis la racine du dépôt) :
  ```bash
  cargo run -p vinx-node        # expose le RPC sur http://127.0.0.1:8545
  ```

## Lancer en développement

Depuis `apps/vinx-desktop/` :

```bash
# (une seule fois) générer les icônes multi-plateformes depuis app-icon.png
cargo tauri icon app-icon.png

# lancer l'app (fenêtre native + rechargement du frontend)
cargo tauri dev
```

L'app s'ouvre, se connecte au nœud indiqué en haut (défaut `http://127.0.0.1:8545`),
et propose un chemin de wallet par défaut dans le dossier de config de l'app.

## Construire un binaire distribuable

```bash
cargo tauri build     # produit un exécutable/installeur dans src-tauri/target/release/
```

## Notes

- **Modèle de sécurité** : le fichier wallet (`{ address, secret_key_hex }`) est lu par
  le backend Rust ; la clé reste dans le processus et sert uniquement à signer
  localement. Le frontend ne voit jamais la clé.
- **Frais** : pour un transfert, l'app récupère le frais de base courant du nœud et
  l'applique automatiquement (forfait indépendant du montant).
- **Chain ID** : récupéré depuis `/health` et lié à chaque signature (anti-replay).
- **CSP** : désactivé (`null`) pour simplifier ce premier scaffold local ; à durcir
  avant toute distribution large.
- Ce dossier est un **workspace Cargo séparé** (exclu du workspace du nœud) pour que
  les dépendances GUI n'affectent ni le build du nœud ni sa CI.
