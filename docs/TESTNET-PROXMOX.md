# Testnet sur Proxmox

Un testnet en deux phases, sur un seul hôte Proxmox :

1. **un validateur**, pour prendre en main le nœud, le wallet, la multisig et la
   supervision ;
2. **quatre validateurs**, pour éprouver le consensus : panne, arrêt, reprise.

Le guide général est [`EXPLOITATION.md`](./EXPLOITATION.md) ; celui-ci en
reprend les étapes, adaptées à Proxmox, et indique quoi tester.

## 0. Avant de commencer

**Un conteneur ou une VM par nœud.** Un conteneur LXC Debian 12 suffit :
2 vCPU, 2 Go de RAM, 20 Go de disque. Pour la supervision (Docker) dans un
LXC, active *Options → Features → nesting*. Donne à chaque conteneur une IP
fixe sur le même pont (ex. `vmbr0`).

**Les snapshots Proxmox sont dangereux pour un validateur.** Restaurer le
snapshot d'un validateur pendant que le réseau tourne le fait repartir d'une
hauteur ancienne, avec un garde anti-double vote ancien : il peut alors signer
un bloc différent à une hauteur déjà finalisée. Avec quatre validateurs, c'est
une équivoque, signalée comme une clé compromise. Utilise les snapshots
uniquement réseau arrêté, et en sachant que tout ce qui a suivi est perdu.

**Les clés admin restent sur ton poste**, jamais dans un conteneur.

## Phase 1 — un validateur

### 1.1 Compiler

Sur ton poste ou dans un conteneur de compilation :

```bash
cargo build --release -p vinx-node -p vinx-wallet
```

Copie `target/release/vinx-node` et `vinx-wallet` dans `/usr/local/bin/` de
chaque conteneur, et `vinx-wallet` sur ton poste.

### 1.2 Créer les clés

```bash
# Sur ton poste : trois clés admin (indices 0, 1, 2 dans l'ordre de la genèse)
export VINX_KEY_PASSWORD='…'
vinx-wallet keygen --out admin-1.json
vinx-wallet keygen --out admin-2.json
vinx-wallet keygen --out admin-3.json

# Dans le conteneur du validateur 1
sudo mkdir -p /etc/vinx
sudo vinx-node keygen --out /etc/vinx/validator.json
```

Note chaque clé publique affichée (`01…`).

### 1.3 Écrire la genèse

Sur ton poste. `--escrow-period 600` fait échoir une tranche d'escrow toutes les
10 minutes au lieu de tous les mois, pour pouvoir tester les libérations ; il
n'est accepté que si l'identifiant de chaîne contient `testnet` ou `devnet`.

```bash
vinx-node genesis --chain-id vinx-testnet-1 \
  --admin <admin-1> --admin <admin-2> --admin <admin-3> \
  --validator <validateur-1> \
  --escrow-period 600 \
  --out genesis.json
```

Copie `genesis.json` dans `/etc/vinx/` du conteneur.

### 1.4 Démarrer le validateur 1 en archive

Installe le service (EXPLOITATION §3.4), avec dans `/etc/vinx/vinx-node.env` :

```bash
VINX_OPTIONS="--archive"
```

**L'archive est indispensable pour la phase 2** : les validateurs ajoutés plus
tard devront rejouer la chaîne depuis la genèse, ce que seul un nœud archive
peut leur fournir (SPEC §8.3).

```bash
sudo systemctl enable --now vinx-node
journalctl -u vinx-node -f        # un « bloc finalisé » toutes les 5 s
```

Depuis ton poste, ouvre un tunnel puis le tableau de bord :

```bash
ssh -L 8545:127.0.0.1:8545 root@<IP du validateur 1>
# http://127.0.0.1:8545
```

### 1.5 Quoi tester

Toutes les commandes `vinx-wallet` se lancent sur ton poste, à travers le tunnel.

| Test | Commandes | Attendu |
|---|---|---|
| Financer un compte | `vinx-wallet keygen --out alice.json`, puis `admin propose … treasury-transfer --to <alice> --amount 100`, deux `admin approve`, `admin submit` | Alice a 100 VINX |
| Paiement | `vinx-wallet send --key alice.json --to <bob> --amount 1.5 --memo test` | Bob a 1,5 VINX ; frais à la trésorerie |
| Réserve minimale | envoyer 0,05 VINX à un compte neuf | refusé (réserve 0,1 VINX) |
| Vider un compte | envoyer tout le solde moins les frais | solde 0, compte conservé |
| Mémo obligatoire | `memo-required --key bob.json on`, puis un paiement sans mémo | refusé |
| Rotation de clé | `rotate-key --key alice.json --new-key alice2.json`, puis `send --key alice2.json --from <alice> …` | accepté ; l'ancienne clé est refusée |
| Une seule approbation | `admin submit` avec une seule approbation | refusé |
| Libérer l'escrow | après 10 min : `admin propose … escrow-release --amount 7500000` | trésorerie + 7,5 M VINX |
| Libérer trop tôt | relancer aussitôt | refusé : tranche suivante non échue |
| Paramètre | `set-param --param min_fee --value 2000000` | frais minimum à 0,002 VINX |
| Coupure brutale | *Stop* du conteneur dans Proxmox, puis *Start* | le nœud repart à la même hauteur |
| Alerte | arrêter le service une minute | tableau de bord « chaîne à l'arrêt », alerte `VinxChaineArretee` |

`vinx-wallet admin status` résume l'état de la gouvernance à tout moment.
Une action admin impossible (tranche non échue, nonce faux, approbations
insuffisantes) est refusée dès `admin submit`, avec la raison.

## Phase 2 — passer à quatre validateurs

### 2.1 Pourquoi les ajouter d'un coup

La chaîne n'avance que si plus des deux tiers des validateurs votent :

| Validateurs | Doivent voter | Pannes tolérées |
|---|---|---|
| 1 | 1 | 0 |
| 2 | 2 | 0 |
| 3 | 3 | 0 |
| 4 | 3 | **1** |

Ajouter les validateurs un par un fait traverser des étapes où un seul nœud
absent bloque la chaîne. On ajoute donc les trois **ensemble**.

### 2.2 Préparer les trois nouveaux conteneurs

Pour chacun (2, 3, 4) : binaires, `genesis.json`, clé de validateur
(`vinx-node keygen`), service. Récupère l'identifiant réseau de chacun des
quatre nœuds :

```bash
sudo install -d -o vinx -g vinx -m 700 /var/lib/vinx
sudo -u vinx vinx-node peer-id --data-dir /var/lib/vinx
```

Dans `/etc/vinx/vinx-node.env` de **chaque** nœud, une option `--peer` par
**autre** nœud :

```bash
VINX_OPTIONS="--peer /ip4/<IP>/tcp/26656/p2p/<id> --peer … --peer …"
# validateur 1 : garder --archive en plus
```

Redémarre le validateur 1 pour qu'il accepte les nouveaux pairs, puis démarre
les trois nouveaux **avec leur clé**. Tant qu'ils ne sont pas dans l'ensemble
de validateurs, ils suivent la chaîne sans voter ; ils la rattrapent depuis
l'archive.

### 2.3 Vérifier avant d'ajouter

Ouvre un tunnel vers chaque nœud, sur des ports différents :

```bash
ssh -L 18541:127.0.0.1:8545 root@<IP 1>   # et 18542, 18543, 18544 pour les autres
```

Puis :

```bash
vinx-wallet --node http://127.0.0.1:18541 preflight \
  --new-node http://127.0.0.1:18542 \
  --new-node http://127.0.0.1:18543 \
  --new-node http://127.0.0.1:18544
```

`preflight` vérifie pour chaque futur validateur : même chaîne, même dernier
bloc que la référence, au plus 3 blocs de retard, au moins un pair, une clé de
validateur chargée et différente des autres. S'il ne signale aucun problème, il
affiche les trois commandes `add-validator` à envoyer, avec leurs nonces.

### 2.4 Ajouter les trois validateurs

Pour chaque fichier affiché par `preflight` : `admin propose`, deux
`admin approve`, `admin submit`, **à la suite**. Deux blocs après leur
inclusion, les trois nœuds se mettent à voter d'eux-mêmes : **aucun
redémarrage** n'est nécessaire. Le tableau de bord de chacun passe alors de
« observateur » à « validateur n° … ».

### 2.5 Quoi tester

| Test | Action | Attendu |
|---|---|---|
| Rythme | observer quelques minutes | 100 % au premier round, un bloc toutes les 5 s |
| Une panne | arrêter un validateur | la chaîne continue |
| Retour | le redémarrer | il rattrape la chaîne |
| Deux pannes | arrêter deux validateurs | la chaîne s'arrête, sans diverger |
| Reprise | en redémarrer un | la chaîne repart |
| Coupure réseau | couper l'interface d'un conteneur | `VinxPairsManquants` chez lui, chaîne intacte chez les autres |
| Transactions pendant une panne | paiements avec un validateur arrêté | finalisés normalement |
| Retirer un validateur | `remove-validator` (puis le réajouter) | à 3 validateurs, tous doivent voter : à faire réseau stable |

Pour réduire l'intervalle de bloc, attends plusieurs jours à 100 % au premier
round, puis `set-param --param min_block_interval --value 4000`.
