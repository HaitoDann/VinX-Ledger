# Déployer un réseau VinX Ledger

Ce guide part de zéro et aboutit à un réseau qui tourne, avec **un seul
validateur** au départ. Il montre ensuite comment passer à quatre. Proxmox, VPS
ou serveur physique : la procédure est la même.

Il y a deux machines :

| Où | Quoi | Outil |
|---|---|---|
| **Ton poste** | les 3 clés admin, la genèse, l'administration | `vinx-wallet`, `vinx-node genesis` |
| **Le serveur** | le validateur, et rien d'autre | `deploy/install-validator.sh` |

Les clés admin ne vont **jamais** sur un serveur.

## 1. Compiler (une fois, sur ton poste)

```bash
cargo build --release -p vinx-node -p vinx-wallet
sudo install target/release/vinx-node target/release/vinx-wallet /usr/local/bin/
```

Le dossier `deploy/` plus le binaire `target/release/vinx-node` suffisent à
installer un serveur.

## 2. Créer les clés admin (une fois, sur ton poste)

```bash
export VINX_KEY_PASSWORD='un mot de passe solide'
vinx-wallet keygen --out admin-1.json
vinx-wallet keygen --out admin-2.json
vinx-wallet keygen --out admin-3.json
```

Note les trois clés publiques affichées (`01…`). Leur ordre compte : ce sont
les signataires 0, 1 et 2. Range une copie de chaque clé sur un support
distinct, dont un hors ligne.

## 3. Préparer le serveur

Serveur : Debian 12 ou Ubuntu LTS, 2 vCPU, 2 Go de RAM, 20 Go de disque, IP
fixe. Sous Proxmox, un conteneur LXC convient.

```bash
# Depuis ton poste :
scp -r deploy target/release/vinx-node root@<IP>:/root/
# Sur le serveur :
mv /root/vinx-node /root/deploy/
sudo /root/deploy/install-validator.sh prepare
```

Le script affiche :

```
Validateur prêt.
  clé publique       01e9b6…
  adresse réseau     /ip4/10.0.0.11/tcp/26656/p2p/12D3KooW…
```

Il a créé l'utilisateur `vinx`, la clé chiffrée du validateur
(`/etc/vinx/validator.json`) et son mot de passe aléatoire
(`/etc/vinx/key-password`). **Sauvegarde ces deux fichiers hors du serveur** :
sans eux, ce validateur ne pourra plus jamais signer.

## 4. Écrire la genèse (sur ton poste)

```bash
vinx-node genesis --chain-id vinx-testnet-1 \
  --admin <admin-1> --admin <admin-2> --admin <admin-3> \
  --validator <clé publique du serveur> \
  --out genesis.json
```

Pour un testnet, ajoute `--escrow-period 600` : une tranche d'escrow est alors
libérable toutes les 10 minutes au lieu de tous les mois. Cette option n'est
acceptée que si l'identifiant de chaîne contient `testnet` ou `devnet`.

La genèse est définitive : la modifier, c'est créer une autre chaîne.

## 5. Démarrer

```bash
scp genesis.json root@<IP>:/root/
# Sur le serveur :
sudo /root/deploy/install-validator.sh start /root/genesis.json --archive
```

Le script installe le service systemd, le démarre et affiche l'état du nœud.
Avec un seul validateur, la chaîne avance immédiatement : un bloc toutes les
5 secondes.

`--archive` garde tout l'historique. Il est nécessaire sur ce premier nœud : les
validateurs ajoutés plus tard rejoueront la chaîne depuis la genèse.

## 6. Observer et administrer (depuis ton poste)

```bash
ssh -N -L 8545:127.0.0.1:8545 root@<IP>      # laisse ce terminal ouvert
```

- Tableau de bord : <http://127.0.0.1:8545>
- `vinx-wallet admin status` : trésorerie, escrow, multisig, validateurs
- Journal : `journalctl -u vinx-node -f` sur le serveur

Une action admin (exemple : 100 VINX de la trésorerie vers Alice) :

```bash
vinx-wallet keygen --out alice.json
vinx-wallet admin propose --out p.json treasury-transfer --to <alice> --amount 100
vinx-wallet admin approve --file p.json --key admin-1.json --index 0
vinx-wallet admin approve --file p.json --key admin-2.json --index 1
vinx-wallet admin submit  --file p.json
```

Une action impossible est refusée dès `submit`, avec sa raison.

### À tester avec un validateur

| Test | Commandes | Attendu |
|---|---|---|
| Paiement | `vinx-wallet send --key alice.json --to <bob> --amount 1.5 --memo test` | Bob a 1,5 VINX |
| Réserve minimale | envoyer 0,05 VINX à un compte neuf | refusé (réserve 0,1 VINX) |
| Mémo obligatoire | `memo-required --key bob.json on`, puis paiement sans mémo | refusé |
| Rotation de clé | `rotate-key --key alice.json --new-key alice2.json` | l'ancienne clé est refusée |
| Une seule approbation | `admin submit` après un seul `approve` | refusé |
| Escrow | après 10 min : `admin propose … escrow-release --amount 7500000` | trésorerie + 7,5 M VINX |
| Escrow trop tôt | relancer aussitôt | refusé : tranche non échue |
| Coupure brutale | arrêter puis relancer le serveur | repart à la même hauteur |
| Alerte | `systemctl stop vinx-node` une minute | tableau de bord « chaîne à l'arrêt » |

## 7. Passer à quatre validateurs

Un seul validateur suffit pour tester. Il ne tolère aucune panne : s'il s'arrête,
la chaîne s'arrête, puis repart avec lui. Quatre validateurs tolèrent une panne.

| Validateurs | Doivent voter | Pannes tolérées |
|---|---|---|
| 1 | 1 | 0 |
| 2 ou 3 | tous | 0 |
| 4 | 3 | **1** |

À 2 ou 3 validateurs, une seule absence bloque la chaîne : on ajoute donc les
trois nouveaux **ensemble**.

1. **Sur chaque nouveau serveur** : section 3 (`prepare`), puis copie le **même**
   `genesis.json`. Note sa clé publique et son adresse réseau.
2. **Relie les quatre nœuds.** Chacun reçoit un `--peer` par **autre** nœud :

   ```bash
   # nœud 1 (garde --archive)
   sudo /root/deploy/install-validator.sh start /root/genesis.json --archive \
     --peer <adresse 2> --peer <adresse 3> --peer <adresse 4>
   # nœud 2 (de même pour 3 et 4)
   sudo /root/deploy/install-validator.sh start /root/genesis.json \
     --peer <adresse 1> --peer <adresse 3> --peer <adresse 4>
   ```

   Les nouveaux nœuds rattrapent la chaîne et la suivent sans voter.
3. **Vérifie** depuis ton poste, avec un tunnel par nœud :

   ```bash
   ssh -N -L 18541:127.0.0.1:8545 root@<IP 1> &   # 18542, 18543, 18544 pour les autres
   vinx-wallet --node http://127.0.0.1:18541 preflight \
     --new-node http://127.0.0.1:18542 \
     --new-node http://127.0.0.1:18543 \
     --new-node http://127.0.0.1:18544
   ```

   `preflight` contrôle chaque nouveau nœud : même chaîne, synchronisé, relié,
   clé chargée. Si tout est bon, il affiche les trois `add-validator` à faire.
4. **Ajoute-les à la suite**, chacun avec `admin propose`, deux `approve`,
   `submit`. Deux blocs plus tard, les trois votent, sans redémarrage.

### À tester avec quatre validateurs

| Test | Attendu |
|---|---|
| Arrêter un validateur | la chaîne continue |
| Le relancer | il rattrape la chaîne |
| En arrêter deux | la chaîne s'arrête, sans diverger |
| En relancer un | la chaîne repart |
| Paiements avec un validateur arrêté | finalisés normalement |

## Pièges

- **Ne restaure pas le snapshot d'un validateur pendant que le réseau tourne.**
  Il repartirait d'une hauteur ancienne et pourrait signer deux blocs différents
  à la même hauteur, ce qui est puni comme une clé compromise. Les snapshots
  Proxmox ne se font que réseau arrêté.
- **Ne copie jamais la clé d'un validateur sur deux serveurs en marche.** Pour
  la même raison.
- **Le port 8545 n'est jamais exposé** : seulement le 26656, ouvert aux IP des
  autres nœuds. Le reste passe par le tunnel SSH.

Pour la supervision Prometheus/Grafana, les alertes et la sécurité en
production, voir [`EXPLOITATION.md`](./EXPLOITATION.md).
