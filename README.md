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
  --genesis devnet/genesis.json --key devnet/validator-1.json --data-dir devnet/data-1
```

`devnet` crée trois clés admin et une clé de validateur, chiffrées par le mot de
passe, ainsi que la genèse. L'API écoute sur `http://127.0.0.1:8545` :
`/status`, `/accounts/{adresse}`, `/blocks/{hauteur}`, `/escrow`, `/fees`,
`/evidence` (preuves de validateurs fautifs), et en `POST`
`/transactions` et `/admin-transactions`.


## Plusieurs validateurs

`devnet --validators 4` crée quatre clés de validateur. Chaque nœud a une
identité réseau, affichée par `vinx-node peer-id --data-dir <dossier>`. Un nœud
n'accepte que les pairs qu'il joint (`--peer`) ou autorise (`--allow`) :

```bash
vinx-node run --genesis devnet/genesis.json --key devnet/validator-1.json \
  --data-dir devnet/data-1 --listen /ip4/0.0.0.0/tcp/26656 \
  --peer /ip4/10.0.0.2/tcp/26656/p2p/<id du nœud 2> \
  --peer /ip4/10.0.0.3/tcp/26656/p2p/<id du nœud 3> \
  --peer /ip4/10.0.0.4/tcp/26656/p2p/<id du nœud 4>
```

Sans `--key`, le nœud suit la chaîne en observateur et sert l'API. La chaîne
avance tant que plus des deux tiers des validateurs sont joignables ; un nœud
qui revient rattrape les blocs manquants automatiquement.

Un nœud ne garde que l'état et les blocs des dernières 24 heures
(`--block-retention-hours`). Un nouveau nœud télécharge directement l'état,
vérifié contre un bloc certifié, au lieu de rejouer toute la chaîne. Gardez au
moins un nœud `--archive`, qui conserve tout l'historique.

## Utiliser le wallet

```bash
alias wallet='cargo run --release -q -p vinx-wallet --'
wallet keygen --out alice.json            # adresse vinx1…
wallet balance <adresse>
wallet send --key alice.json --to <adresse> --amount 12.5 --memo "FAC-1"
wallet memo-required --key boutique.json on
```

Les actions de la trésorerie et de l'escrow passent par une proposition que
deux des trois signataires approuvent, chacun avec sa propre clé :

```bash
wallet admin propose --out p.json treasury-transfer --to <adresse> --amount 250
wallet admin approve --file p.json --key devnet/admin-1.json --index 0
wallet admin approve --file p.json --key devnet/admin-2.json --index 1
wallet admin submit --file p.json
```
