# Déployer un réseau VinX Ledger

Ce guide part de zéro : un premier validateur, puis quatre. Proxmox, VPS ou
serveur physique, la procédure est la même.

## 0. Les règles à connaître avant de commencer

**Chaque commande dit où elle se tape.** Il y a deux sortes de machines :

| Machine | Ce qu'elle contient | Ce qu'on y tape |
|---|---|---|
| 🖥️ **Le poste** (ton PC Windows, macOS ou Linux) | les 3 clés admin, la genèse | `vinx-wallet admin …`, `vinx-node genesis` |
| 🗄️ **Un serveur** (un par validateur) | sa clé de validateur, et rien d'autre | `install-validator.sh`, `journalctl`, `systemctl` |

- Les commandes `vinx-wallet admin` ne marchent **que sur le poste** : c'est là
  que sont les clés admin. Ailleurs, elles répondent « fichier de clé
  introuvable ».
- Les clés admin ne vont **jamais** sur un serveur.

**Les valeurs en MAJUSCULES sont à remplacer**, en entier, par ta valeur :
`IP_DU_SERVEUR` devient `192.168.1.233`. N'ajoute ni guillemets ni chevrons
`< >` : pour le terminal, `<` veut dire « lire un fichier ».

**Colle les commandes une par une**, ou un bloc à la fois en attendant la fin du
collage. Si le terminal affiche `>` au lieu de l'invite habituelle, une
commande est restée ouverte (souvent un guillemet) : `Ctrl+C`, puis recommence.

## 1. Récupérer les binaires

Chaque version est publiée dans l'onglet *Releases* du dépôt GitHub :

| Archive | Pour |
|---|---|
| `vinx-VERSION-linux-x86_64.tar.gz` | 🗄️ les serveurs (Debian 12, Ubuntu 22.04 ou plus récents) |
| `vinx-VERSION-windows-x86_64.zip` | 🖥️ un poste Windows |
| `vinx-VERSION-macos-arm64.tar.gz` | 🖥️ un Mac à puce Apple |

🗄️ **Sur chaque serveur**, en `root` :

```bash
apt update && apt install -y curl ca-certificates openssh-server
cd /root && V=0.3.0
curl -LO https://github.com/HaitoDann/VinX-Ledger/releases/download/v$V/vinx-$V-linux-x86_64.tar.gz
curl -LO https://github.com/HaitoDann/VinX-Ledger/releases/download/v$V/SHA256SUMS
sha256sum -c --ignore-missing SHA256SUMS      # doit afficher : OK
tar xzf vinx-$V-linux-x86_64.tar.gz && mv vinx-$V-linux-x86_64 vinx
install vinx/vinx-node vinx/vinx-wallet /usr/local/bin/
```

🖥️ **Sur le poste Windows** : télécharge le `.zip` depuis la page de la
release, décompresse-le (par exemple dans `C:\vinx`) et ouvre PowerShell dans
ce dossier. Les commandes s'écrivent `.\vinx-wallet.exe …` et le mot de passe
se donne par `$env:VINX_KEY_PASSWORD = 'mot de passe'`. Sur macOS ou Linux :
`./vinx-wallet …` et `export VINX_KEY_PASSWORD='mot de passe'`.

Les exemples ci-dessous utilisent l'écriture Linux ; adapte-les sur Windows.

## 2. Créer les clés admin

🖥️ **Sur le poste**, une seule fois :

```bash
export VINX_KEY_PASSWORD='un mot de passe solide'
vinx-wallet keygen --out admin-1.json
vinx-wallet keygen --out admin-2.json
vinx-wallet keygen --out admin-3.json
```

Note les trois **clés** affichées (`01…`, 66 caractères), dans l'ordre : ce sont
les signataires 0, 1 et 2. Range une copie de chaque fichier sur un support
distinct, dont un hors ligne.

Une clé (`01…`) n'est pas une adresse (`vinx1…`). Les paiements vont à une
**adresse** ; la genèse et `add-validator` prennent une **clé**.

## 3. Préparer le premier serveur

🗄️ **Sur le serveur 1** :

```bash
/root/vinx/install-validator.sh prepare
```

```
Validateur prêt.
  clé publique       01e9b6…
  adresse réseau     /ip4/192.168.1.233/tcp/26656/p2p/12D3KooW…
```

Note la **clé publique** et l'**adresse réseau**. Le script a créé
l'utilisateur `vinx`, la clé chiffrée du validateur (`/etc/vinx/validator.json`)
et son mot de passe aléatoire (`/etc/vinx/key-password`). **Sauvegarde ces deux
fichiers hors du serveur** : sans eux, ce validateur ne pourra plus signer.

Pour retrouver plus tard ces deux valeurs : relance `prepare`, qui ne touche à
rien de ce qui existe déjà.

## 4. Écrire la genèse

🖥️ **Sur le poste**, avec les 3 clés admin et la clé du serveur 1 :

```bash
vinx-node genesis --chain-id vinx-testnet-1 \
  --admin CLE_ADMIN_1 --admin CLE_ADMIN_2 --admin CLE_ADMIN_3 \
  --validator CLE_SERVEUR_1 \
  --escrow-period 600 \
  --out genesis.json
scp genesis.json root@IP_SERVEUR_1:/root/
```

`--escrow-period 600` rend une tranche d'escrow libérable toutes les 10 minutes
au lieu de tous les mois, pour pouvoir le tester. Il n'est accepté que si
l'identifiant de chaîne contient `testnet` ou `devnet` : ne le mets pas sur un
vrai réseau.

La genèse est définitive : la modifier, c'est créer une autre chaîne.

## 5. Démarrer le premier validateur

🗄️ **Sur le serveur 1** :

```bash
/root/vinx/install-validator.sh start /root/genesis.json --archive
```

Le script installe le service systemd, le démarre et affiche l'état du nœud.
Avec un seul validateur, la chaîne avance aussitôt : un bloc toutes les 5 s.

```bash
journalctl -u vinx-node -f        # « bloc finalisé » toutes les 5 s ; Ctrl+C pour quitter
```

`--archive` garde tout l'historique. Il est indispensable sur ce premier nœud :
les validateurs ajoutés plus tard rejoueront la chaîne depuis la genèse.

## 6. Administrer et observer depuis le poste

L'API du nœud n'écoute qu'en local, par sécurité. 🖥️ **Sur le poste**, ouvre un
tunnel SSH et **laisse ce terminal ouvert** :

```bash
ssh -N -L 8545:127.0.0.1:8545 root@IP_SERVEUR_1
```

(Sur Debian, l'accès root par mot de passe est fermé par défaut. Sur le
serveur : `passwd`, puis
`sed -i 's/^#\?PermitRootLogin.*/PermitRootLogin yes/' /etc/ssh/sshd_config`
et `systemctl restart ssh`. Préfère une clé SSH dès que possible.)

Ensuite, dans un autre terminal du poste :

- tableau de bord : <http://127.0.0.1:8545> ;
- `vinx-wallet admin status` : trésorerie, escrow, multisig, validateurs.

Une action admin, par exemple 100 VINX de la trésorerie vers Alice :

```bash
vinx-wallet keygen --out alice.json            # affiche l'adresse vinx1… d'Alice
vinx-wallet admin propose --out p1.json treasury-transfer --to ADRESSE_ALICE --amount 100
vinx-wallet admin approve --file p1.json --key admin-1.json --index 0
vinx-wallet admin approve --file p1.json --key admin-2.json --index 1
vinx-wallet admin submit  --file p1.json
```

Chaque proposition porte un numéro d'ordre (nonce) utilisable une seule fois :
**un nouveau fichier par action** (`p1.json`, `p2.json`…). Une action
impossible est refusée dès `submit`, avec sa raison.

### À tester avec un validateur

| Test | Commandes (🖥️ poste) | Attendu |
|---|---|---|
| Paiement | `vinx-wallet send --key alice.json --to ADRESSE_BOB --amount 1.5 --memo test` | Bob a 1,5 VINX |
| Réserve minimale | envoyer 0,05 VINX à un compte neuf | refusé (réserve 0,1 VINX) |
| Mémo obligatoire | `memo-required --key bob.json on`, puis paiement sans mémo | refusé |
| Rotation de clé | `rotate-key --key alice.json --new-key alice2.json` | l'ancienne clé est refusée |
| Une seule approbation | `admin submit` après un seul `approve` | refusé |
| Escrow | après 10 min : `admin propose … escrow-release --amount 7500000` | trésorerie + 7,5 M VINX |
| Escrow trop tôt | relancer aussitôt | refusé : tranche non échue |
| Coupure brutale | 🗄️ arrêter puis relancer le serveur | repart à la même hauteur |
| Alerte | 🗄️ `systemctl stop vinx-node` une minute | tableau de bord « chaîne à l'arrêt » |

## 7. Passer à quatre validateurs

Un validateur ne tolère aucune panne. Quatre en tolèrent une :

| Validateurs | Doivent voter | Pannes tolérées |
|---|---|---|
| 1 | 1 | 0 |
| 2 ou 3 | tous | 0 |
| 4 | 3 | **1** |

À 2 ou 3 validateurs, une seule absence bloque la chaîne : on ajoute donc les
trois nouveaux **à la suite**, sans pause.

### 7.1 Créer trois nouveaux serveurs

Crée-les **neufs** (sous Proxmox : depuis le modèle Debian, 2 vCPU, 2 Go de
RAM, 20 Go de disque, IP fixe). **Ne clone jamais un validateur** : le clone
aurait la même clé, et deux nœuds avec la même clé finissent par signer deux
fois, ce qui est puni comme une clé compromise.

🗄️ **Sur chacun des trois** : section 1 (serveur), puis :

```bash
/root/vinx/install-validator.sh prepare
scp root@IP_SERVEUR_1:/root/genesis.json /root/genesis.json
```

Note la **clé publique** et l'**adresse réseau** de chacun.

### 7.2 Relier les quatre nœuds

Chaque nœud reçoit un `--peer` par **autre** nœud, avec l'adresse réseau
complète affichée par `prepare` (`/ip4/…/tcp/26656/p2p/12D3KooW…`). Le script
refuse une adresse mal formée sans toucher au nœud.

🗄️ **Sur le serveur 1** (il garde `--archive`) :

```bash
/root/vinx/install-validator.sh start /root/genesis.json --archive \
  --peer ADRESSE_RESEAU_2 --peer ADRESSE_RESEAU_3 --peer ADRESSE_RESEAU_4
```

🗄️ **Sur le serveur 2** (de même sur 3 et 4, avec les trois autres adresses) :

```bash
/root/vinx/install-validator.sh start /root/genesis.json \
  --peer ADRESSE_RESEAU_1 --peer ADRESSE_RESEAU_3 --peer ADRESSE_RESEAU_4
```

Les nouveaux nœuds rattrapent la chaîne depuis le serveur 1, puis la suivent
sans voter. Vérifie, 🗄️ **sur chacun des quatre** :

```bash
curl -s 127.0.0.1:8545/status | grep -o '"height":[0-9]*\|"peers":[0-9]*'
```

Attendu : la même hauteur partout (à 1 ou 2 blocs près) et `"peers":3`.

### 7.3 Ajouter les trois validateurs

🖥️ **Sur le poste** (tunnel vers le serveur 1 ouvert), pour chaque nouveau
serveur **à la suite**, en attendant 10 secondes entre deux :

```bash
vinx-wallet admin propose --out add-2.json add-validator --key CLE_SERVEUR_2
vinx-wallet admin approve --file add-2.json --key admin-1.json --index 0
vinx-wallet admin approve --file add-2.json --key admin-2.json --index 1
vinx-wallet admin submit  --file add-2.json
```

Puis la même chose avec `add-3.json` / `CLE_SERVEUR_3` et `add-4.json` /
`CLE_SERVEUR_4`. Deux blocs après chaque ajout, le nouveau validateur vote de
lui-même, sans redémarrage. `vinx-wallet admin status` doit lister quatre
validateurs, et dans le tableau de bord la colonne **Proposeur** tourne entre 1,
2, 3 et 4.

Les numéros de validateur suivent l'**ordre d'ajout**, pas le nom des
serveurs : note la correspondance.

Avant l'ajout, `vinx-wallet preflight --new-node …` peut contrôler chaque
nouveau nœud (même chaîne, synchronisé, relié, clé chargée), si leurs API sont
joignables par des tunnels.

### 7.4 À tester avec quatre validateurs

| Test | Action | Attendu |
|---|---|---|
| Une panne | 🗄️ `systemctl stop vinx-node` sur un serveur | la chaîne continue ; ses tours passent en round 1 (environ 9 s) |
| Retour | 🗄️ `systemctl start vinx-node` | il rattrape la chaîne, `"peers":3` |
| Deux pannes | arrêter deux serveurs | la chaîne s'arrête ; même hauteur et même `last_block_hash` sur les deux restants |
| Reprise | en relancer un | la chaîne repart d'elle-même |
| Charge | section 9 | environ 1 000 transactions/s, des transactions dans les blocs des quatre proposeurs |

## 8. Mettre à jour

🗄️ **Sur chaque serveur**, l'un après l'autre :

```bash
cd /root && V=NOUVELLE_VERSION
curl -LO https://github.com/HaitoDann/VinX-Ledger/releases/download/v$V/vinx-$V-linux-x86_64.tar.gz
curl -LO https://github.com/HaitoDann/VinX-Ledger/releases/download/v$V/SHA256SUMS
sha256sum -c --ignore-missing SHA256SUMS && tar xzf vinx-$V-linux-x86_64.tar.gz
/root/vinx-$V-linux-x86_64/install-validator.sh upgrade
install /root/vinx-$V-linux-x86_64/vinx-wallet /usr/local/bin/
```

Le script :

1. arrête le nœud et sauvegarde le binaire et les données dans
   `/var/backups/vinx/` (les trois dernières sauvegardes sont gardées) ;
2. installe le nouveau binaire et redémarre ;
3. attend qu'un nouveau bloc soit finalisé, deux minutes au plus ;
4. sinon, **revient de lui-même à l'ancien binaire** et le dit.

`…/install-validator.sh rollback` revient à l'ancien binaire à la main. Le
tableau de bord indique la version (« Logiciel »).

- **Avec un validateur**, la chaîne s'arrête quelques secondes, puis repart.
- **Avec quatre**, un serveur à la fois : attends « mise à jour terminée »
  avant de passer au suivant. La chaîne continue pendant ce temps.
- Lis les notes de la version : certaines demandent de ne pas envoyer de
  transactions tant que tous les nœuds ne sont pas à jour.
- **Le retour arrière ne restaure que le binaire**, jamais les données : elles
  gardent la mémoire des votes déjà signés, ce qui empêche le validateur de
  voter deux fois à la même hauteur.
- Une sauvegarde de données ne se restaure que **réseau arrêté** (voir
  « Pièges »).

N'oublie pas ton poste : télécharge aussi la nouvelle archive Windows ou macOS.

## 9. Mesurer le débit (test de charge)

`vinx-wallet bench` crée des comptes jetables, les finance depuis un compte
existant, leur fait s'envoyer des montants minimes, puis compte les
transactions **finalisées** (pas seulement envoyées).

🗄️ **Lance-le sur un serveur**, pas à travers le tunnel : sinon, c'est le
tunnel qu'on mesure. Copie-y la clé d'un compte de test (jamais une clé
admin) :

```bash
# 🖥️ sur le poste
scp alice.json root@IP_SERVEUR_1:/root/
# 🗄️ sur le serveur 1 ; le compte doit avoir un peu plus de 200 VINX
export VINX_KEY_PASSWORD='mot de passe de alice.json'
vinx-wallet bench --key /root/alice.json --accounts 200 --duration 60
```

```
envoyées         73000
refusées         0
finalisées       73000 en 15 bloc(s), 75.0 s
débit moyen      973 transactions/s
bloc le plus plein  5000 transactions
```

- **Le plafond du protocole** est `max_block_txs / intervalle de bloc` :
  5 000 / 5 s = **1 000 transactions/s** avec les paramètres par défaut. Si
  « bloc le plus plein » vaut 5 000, c'est ce plafond qu'on a atteint.
- **Pour trouver le plafond de la machine**, relève la limite sur un réseau de
  test (`admin propose … set-param --param max_block_txs --value 20000`), puis
  relance avec `--accounts 220 --fund 3` (environ 700 VINX). Quand les blocs
  mettent nettement plus de 5 s à venir (`journalctl`), c'est le nœud qui
  sature. Repère : environ 3 000 transactions/s sur un validateur seul.
- Remets ensuite `max_block_txs` à 5000 : des blocs trop gros allongent la
  finalité et le rattrapage des nœuds.

## Pièges

- **Ne restaure pas le snapshot d'un validateur pendant que le réseau tourne.**
  Il repartirait d'une hauteur ancienne et pourrait signer deux blocs
  différents à la même hauteur, ce qui est puni comme une clé compromise. Les
  snapshots Proxmox ne se font que réseau arrêté.
- **Ne clone pas un validateur, ne copie pas sa clé sur un autre serveur en
  marche.** Pour la même raison.
- **Le port 8545 n'est jamais exposé** : seul le 26656 l'est, et seulement aux
  IP des autres nœuds. Le reste passe par le tunnel SSH.

Pour la supervision Prometheus/Grafana, les alertes et la sécurité en
production, voir [`EXPLOITATION.md`](./EXPLOITATION.md).
