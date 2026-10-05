# VinX Wallet — extension de navigateur

Portefeuille VinX dans le navigateur (Chrome, Edge, Brave, Firefox). Les mêmes 12 mots
donnent la même adresse que l'application de bureau et `vinx-wallet`.

## Installer (mode développeur)

**Chrome / Edge / Brave** : ouvrir `chrome://extensions`, activer « Mode développeur »,
cliquer « Charger l'extension non empaquetée » et choisir ce dossier `apps/vinx-extension`.

**Firefox** : ouvrir `about:debugging#/runtime/this-firefox`, « Charger un module
complémentaire temporaire… » et choisir `manifest.json`.

## Utiliser

1. Créer un portefeuille (noter les 12 mots) ou en importer un.
2. Choisir un mot de passe (8 caractères minimum) : la clé est chiffrée localement
   (Argon2id 19 Mio, comme l'appli de bureau → AES-256-GCM) et se verrouille après 15 min.
   Un ancien coffre PBKDF2 est rechiffré en Argon2id au premier déverrouillage.
3. Réglages → adresse du nœud (par défaut `http://127.0.0.1:8545`, par exemple
   `http://192.168.1.231:8545` pour un validateur du réseau local).
4. Envoyer : coller une adresse ou un URI `vinx:<adresse>?amount=12.40&memo=REF`
   (montant et référence sont remplis automatiquement), puis confirmer.

## Tester

```sh
node apps/vinx-extension/test.mjs
```

Vérifie l'adresse dérivée d'une phrase contre le vecteur Rust, le coffre chiffré et les URI.

## Archive prête à installer

Le workflow `extension.yml` teste l'extension et produit `vinx-wallet-<version>.zip`
(artefact du workflow, joint à la Release sur un tag `v*`). Décompresser l'archive puis
la charger comme ci-dessus, ou la soumettre telle quelle au Chrome Web Store / AMO.
