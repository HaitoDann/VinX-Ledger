# VinX — application de bureau

Un portefeuille pour tout le monde, et un interrupteur pour devenir validateur.
Windows, macOS et Linux, sans Docker ni terminal.

## Pour l'utilisateur

1. Installer l'application (`.msi` / `.exe`, `.dmg`, `.deb` / `.AppImage`).
2. **Créer un portefeuille** : l'application affiche 12 mots à noter sur papier, puis
   demande un mot de passe pour cet ordinateur. Ou **restaurer** avec ses 12 mots.
3. **Accueil** : solde, historique, *VINX de test* (faucet du testnet).
   **Envoyer** (avec référence facultative), **Recevoir** (adresse et QR code).
4. **Valider** : un interrupteur. L'application lance le validateur, dépose la garantie
   minimale du réseau et montre les étapes : nœud synchronisé → garantie → échauffement
   (environ 3 h) → actif. L'arrêter propose une pause ou le retrait de la garantie
   (21 jours de délai).
5. **Réglages** : le point d'entrée du réseau (un nom ou une IP ; un seul suffit).

Le validateur continue de tourner quand la fenêtre est fermée, et redémarre avec
l'application.

## Sécurité

- Le portefeuille est chiffré sur le disque (Argon2id + AES-256-GCM), au même format que
  `vinx-wallet` : un fichier, ou les 12 mots, s'ouvrent dans les deux.
- La clé ne quitte jamais le processus Rust : l'interface web n'y a pas accès.
- Le validateur a **ses propres clés** (opérateur et BLS, ADR 0084 S5). Le portefeuille
  signe seulement la garantie qui le désigne : la clé des fonds n'est jamais écrite en
  clair pour le nœud.

## Architecture

```
apps/vinx-desktop/
├── src-tauri/src/main.rs   commandes : portefeuille, envoi, faucet, validateur
├── src-tauri/src/node.rs   le nœud embarqué : lancement détaché, clés, genèse
├── ui/                     interface (HTML/CSS/JS, même charte que l'interface web)
└── app-icon.png
crates/vinx-desktop-core    phrase de récupération, fichier chiffré, transactions (testé en CI)
```

Fichiers de l'utilisateur (dossier de données de l'application) : `wallet.json`,
`settings.json`, `genesis.json`, `node/` (données et clés du validateur), `node.log`.

## Développement

```bash
sudo apt install libwebkit2gtk-4.1-dev librsvg2-dev          # Linux
cargo build --release -p vinx-node                            # le nœud embarqué
mkdir -p apps/vinx-desktop/src-tauri/binaries
cp target/release/vinx-node apps/vinx-desktop/src-tauri/binaries/vinx-node-$(rustc -vV | sed -n 's/^host: //p')
cd apps/vinx-desktop && npx @tauri-apps/cli@2 dev
```

Test de bout en bout (restauration, faucet, validateur, paiement) contre un réseau :

```bash
./vinx start --lan                                            # à la racine du dépôt
cd apps/vinx-desktop/src-tauri && cargo build
VINX_DESKTOP_SELFTEST=127.0.0.1:8545 ./target/debug/vinx-desktop   # sortie 0 = réussi
```

Les installateurs sont produits par `.github/workflows/desktop.yml` (lancement manuel
ou tag `v*`).
