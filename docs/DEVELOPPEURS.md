# VinX Ledger pour les développeurs

Compiler, lancer un réseau local et utiliser la ligne de commande. Les règles
exactes du protocole sont dans [`SPEC.md`](../SPEC.md).

## Lancer un réseau de développement

Prérequis : [Rust](https://rustup.rs) stable.

```bash
export VINX_KEY_PASSWORD='un mot de passe solide'
cargo run --release -p vinx-node -- devnet --dir devnet
cargo run --release -p vinx-node -- run \
  --genesis devnet/genesis.json --key devnet/validator-1.json --data-dir devnet/data-1
```

`devnet` crée trois clés admin et une clé de validateur, chiffrées par le mot de
passe, ainsi que la genèse. Le tableau de bord du nœud est sur
`http://127.0.0.1:8545/`, l'explorateur sur `/explorer`, ses métriques
Prometheus sur `/metrics`.

## L'API

Chaque nœud décrit son API sur **`/docs`** : conventions (montants en unités,
erreurs, limites), chaque point d'accès avec ses paramètres, un exemple de
réponse et un bouton « Essayer ». La même description, au format OpenAPI 3,
est sur **`/openapi.json`** : elle s'importe dans Postman ou Insomnia, ou sert
à générer un client dans le langage de votre choix.

Sur le testnet : <https://vinx-ledger.vinxlabs.fr/docs>.

Pour suivre un jeton : `/assets/{symbole}`, `/assets/{symbole}/holders` et
`/assets/{symbole}/transactions` (voir [`JETONS.md`](./JETONS.md)).

Les transactions s'envoient en `POST /transactions`, signées et encodées en
Borsh (format dans [`SPEC.md`](../SPEC.md)) : signez-les avec `vinx-wallet` ou
le module WebAssembly du portefeuille mobile (`crates/vinx-mobile`).

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

Pour tout le monde : `vinx-wallet` lancé sans argument (double-clic sous
Windows) ouvre le portefeuille dans le navigateur. Voir
[`PORTEFEUILLE.md`](./PORTEFEUILLE.md).

En ligne de commande :

```bash
alias wallet='cargo run --release -q -p vinx-wallet --'
wallet keygen --out alice.json            # adresse vinx1…
wallet balance <adresse>
wallet send --key alice.json --to <adresse> --amount 12.5 --memo "FAC-1"
wallet memo-required --key boutique.json on
```

Pour administrer à la souris, `wallet cockpit` ouvre une interface locale sur
`http://127.0.0.1:8600` (voir le guide de déploiement, section 6).

Les actions de la trésorerie et de l'escrow passent par une proposition que
deux des trois signataires approuvent, chacun avec sa propre clé :

```bash
wallet admin propose --out p.json treasury-transfer --to <adresse> --amount 250
wallet admin approve --file p.json --key devnet/admin-1.json --index 0
wallet admin approve --file p.json --key devnet/admin-2.json --index 1
wallet admin submit --file p.json
```

## Tests de robustesse

À chaque PR, la CI rejoue automatiquement :

- **les exercices de panne** (`crates/vinx-node/tests/network.rs`, `sync.rs`) :
  quatre validateurs réels en local ; un validateur tombe et la chaîne
  continue, puis il rattrape ; deux tombent et la chaîne s'arrête, puis
  repart à leur retour ; les quatre s'arrêtent brutalement ensemble et
  repartent depuis leurs disques, sans équivoque ; un nouveau nœud rejoint par
  instantané ou depuis un nœud archive ;
- **le fuzzing rapide** :
  - `tests/decoding.rs` : messages réseau et transactions aléatoires ou
    mutés (protocoles 1 à 6), jamais de plantage, encodage canonique seul
    accepté ;
  - `crates/vinx-state/tests/fuzz_assets.rs` : suites aléatoires
    d'opérations sur les jetons, correctement signées ; la supply de chaque
    jeton reste égale à ses soldes plus ses séquestres, et l'état relu
    redonne la même racine ;
  - `tests/node.rs` (`the_public_api_survives_garbage`) : 3 000 requêtes
    absurdes à l'API, jamais d'erreur interne.

### Fuzzing long

Chaque nuit, `.github/workflows/fuzz.yml` lance [cargo-fuzz](https://rust-fuzz.github.io/book/)
10 minutes par cible (`fuzz/fuzz_targets/`) : `net_message` (tout message d'un
pair) et `transaction` (toute transaction, appliquée au dernier protocole).
En local :

```bash
rustup toolchain install nightly && cargo install cargo-fuzz
cd fuzz && cargo +nightly fuzz run net_message -- -max_total_time=600
```

Un plantage laisse l'entrée fautive dans `fuzz/artifacts/` (en CI : artefact
« crashes » de l'exécution) ; `cargo +nightly fuzz run CIBLE FICHIER` la
rejoue.
