<p align="center">
  <img src="./assets/vinx-logo.png" alt="VinX" width="540" />
</p>

<h1 align="center">VinX Ledger</h1>

Un protocole de paiement volontairement simple et minimaliste : validateurs autorisés, supply fixe, émission plafonnée.

> **Statut : expérimental, en reconstruction.** Le token VINX n'a aucune valeur.

- Spécification du protocole : [`SPEC.md`](./SPEC.md)
- Décisions d'architecture : [`docs/adr/`](./docs/adr/)
- Ancien projet (archivé) : `VinX-Ledger-Legacy`
- Licence : [Apache-2.0](./LICENSE)

## Lancer un réseau de développement

Prérequis : [Rust](https://rustup.rs) stable.

```bash
export VINX_KEY_PASSWORD='un mot de passe solide'
cargo run --release -p vinx-node -- devnet --dir devnet
cargo run --release -p vinx-node -- run \
  --genesis devnet/genesis.json --key devnet/validator.json --data-dir devnet/data
```

`devnet` crée trois clés admin et une clé de validateur, chiffrées par le mot de
passe, ainsi que la genèse. L'API écoute sur `http://127.0.0.1:8545` :
`/status`, `/accounts/{adresse}`, `/blocks/{hauteur}`, `/escrow`, et en `POST`
`/transactions` et `/admin-transactions`.

