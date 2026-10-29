# Lancer le testnet 2 (VinX 0.18)

Ce guide remplace l'ancien testnet (protocole 2, version 0.8.1) par une
nouvelle chaîne, `vinx-testnet-2`. Elle démarre directement au **protocole 5**,
avec l'émission des validateurs, les paiements groupés, le multisig, les
mandats, les frais payés par un parrain et les jetons.

Durée : environ une heure. Les étapes 2 à 5 se font à la suite, sans attendre.

| Machine | Adresse | Rôle |
|---|---|---|
| Validateur 1 | 192.168.1.22 | validateur |
| Validateur 2 | 192.168.1.21 | validateur |
| Validateur 3 | 192.168.1.23 | validateur |
| Validateur 4 | 192.168.1.233 | validateur |
| `vinx-public` | 192.168.1.244 | nœud public (archive), `https://vinx-ledger.vinxlabs.fr` |
| Ton poste | | clés admin, portefeuille, cockpit |

## 0. Ce qui change, ce qui reste

- **Effacé** : soldes, noms, séquestres et horodatages de l'ancienne chaîne.
  Chaque nœud en garde une sauvegarde dans `/var/backups/vinx/`.
- **Conservé** : clés admin, clés des validateurs, identités réseau (donc les
  `--peer` et `--allow`), options des nœuds (archive, noms des validateurs),
  configuration Caddy et Livebox. Les adresses `vinx1…` ne changent pas.
- **Nouveau** :
  - la trésorerie démarre avec un robinet de **1 000 000 VINX** pour financer
    les testeurs ;
  - les validateurs reçoivent l'émission et les frais **toutes les heures**
    (époques de 720 blocs).

## 1. Publier la release 0.18.0

Sur GitHub : **Releases → Draft a new release**.

- **Tag** : `v0.18.0`, à créer sur `main`.
- **Titre** : `VinX Ledger 0.18.0`.
- **Description** : colle les notes de version fournies avec ce guide.

Publie. Le workflow **Release** (onglet Actions) compile les archives en
environ 15 minutes, puis les ajoute à la release :

- `vinx-0.18.0-linux-x86_64.tar.gz` pour les serveurs ;
- `vinx-0.18.0-windows-x86_64.zip` (ou macOS) pour ton poste ;
- `SHA256SUMS`.

Attends qu'elles apparaissent avant de continuer.

## 2. Mettre à jour ton poste

Télécharge l'archive Windows (ou macOS) et décompresse-la à la place de
l'ancienne version. Vérifie :

```
vinx-node --version     → vinx-node 0.18.0
vinx-wallet --version   → vinx-wallet 0.18.0
```

## 3. Écrire la genèse (sur ton poste)

Rassemble les **clés publiques** ; aucune clé privée ne quitte sa machine.

- **Admin**, dans le dossier de tes clés :

  ```
  vinx-wallet address --key admin-1.json
  vinx-wallet address --key admin-2.json
  vinx-wallet address --key admin-3.json
  ```

  Garde la ligne « clé » de chacune (elle commence par `01`).
- **Validateurs** : sur chacun des 4 serveurs, `cat /etc/vinx/validator.pub`.
  Note bien l'ordre : validateur 1 = `.22`, 2 = `.21`, 3 = `.23`, 4 = `.233`.

Puis, sur une seule ligne (sous Windows, remplace les `\` par des espaces) :

```
vinx-node genesis --chain-id vinx-testnet-2 \
  --admin CLE-ADMIN-1 --admin CLE-ADMIN-2 --admin CLE-ADMIN-3 \
  --validator CLE-VAL-1 --validator CLE-VAL-2 --validator CLE-VAL-3 --validator CLE-VAL-4 \
  --epoch-blocks 720 --faucet 1000000 \
  --out genesis.json
```

Vérifie le fichier :

- `"chain_id": "vinx-testnet-2"` ;
- `"protocol_version": 5` ;
- les 4 validateurs, dans l'ordre.

## 4. Copier sur les 5 serveurs

Copie `genesis.json` et l'archive Linux sur chaque serveur, puis décompresse
l'archive. Par exemple, depuis ton poste :

```
for ip in 22 21 23 233 244; do
  scp genesis.json vinx-0.18.0-linux-x86_64.tar.gz root@192.168.1.$ip:/root/
  ssh root@192.168.1.$ip "cd /root && tar xzf vinx-0.18.0-linux-x86_64.tar.gz"
done
```

L'archive se décompresse dans `/root/vinx-0.18.0-linux-x86_64/`. Elle contient
`vinx-node` et `install-validator.sh`.

## 5. Basculer les nœuds

Sur chaque serveur, **les 4 validateurs d'abord, `vinx-public` en dernier** :

```
cd /root/vinx-0.18.0-linux-x86_64
sudo ./install-validator.sh reset-chain /root/genesis.json
```

La commande :

1. installe le binaire 0.18.0 ;
2. sauvegarde l'ancienne chaîne ;
3. efface les données en gardant l'identité réseau ;
4. installe la genèse et redémarre le nœud.

Elle affiche ensuite le `/status` du nœud.

Enchaîne les 4 validateurs en quelques minutes : la chaîne démarre dès que
**3 validateurs sur 4** tournent. Un nœud seul reste à la hauteur 0, c'est
normal.

**Si tu te trompes de machine** : `reset-chain` sur `vinx-public` avant les
validateurs ne casse rien. Le nœud attend simplement que la chaîne démarre.

## 6. Vérifier

- **Tableau de bord** `https://vinx-ledger.vinxlabs.fr/` :
  - chaîne `vinx-testnet-2`, protocole 5 ;
  - hauteur qui avance d'un bloc toutes les 5 s ;
  - 4 validateurs verts, en version 0.18.0.
- **Depuis ton poste** :

  ```
  vinx-wallet --node https://vinx-ledger.vinxlabs.fr status
  ```

- **Trésorerie** : `/governance` (ou le cockpit) affiche 1 000 000 VINX.
- **Après une heure** (bloc 720) : la carte **Émission** du tableau de bord
  montre la première époque versée. Chaque validateur a reçu sa part, au
  prorata des blocs qu'il a proposés.

## 7. Premiers essais (checklist)

1. **Cockpit** (`vinx-wallet cockpit`), carte **Trésorerie** : un transfert
   vers ton portefeuille, par exemple 1 000 VINX. Signe avec deux clés admin,
   puis envoie.
2. **Portefeuille** (`vinx-wallet app`, réglages : nœud
   `https://vinx-ledger.vinxlabs.fr`) :
   - [ ] recevoir les VINX, envoyer un paiement, puis un envoi groupé ;
   - [ ] réserver un nom, par exemple `eurv.vinx` ;
   - [ ] **Jetons** : créer `EURV` (100 VINX), l'accepter depuis un deuxième
     compte, l'envoyer, en émettre ;
   - [ ] séquestre en EURV, versé au vendeur ;
   - [ ] mandat en EURV, puis prélèvement ;
   - [ ] horodater un document, puis le vérifier ;
   - [ ] règles du compte : plafond journalier, puis dépense refusée au-delà.
3. **Note tout ce qui surprend** (message d'erreur, lenteur, affichage) :
   c'est le but de ce testnet.

## En cas de problème

| Symptôme | Cause probable | Que faire |
|---|---|---|
| Hauteur bloquée à 0 | Moins de 3 validateurs démarrés | Lancer `reset-chain` sur les validateurs manquants |
| `reset-chain` refuse la genèse | `chain_id` sans « testnet » | Réécrire la genèse avec `--chain-id vinx-testnet-2` |
| Un validateur reste seul | Il n'a pas reçu la bonne genèse | Comparer `sha256sum /etc/vinx/genesis.json` sur les 5 machines |
| Le portefeuille parle de « mauvais identifiant de chaîne » | Ancienne version du portefeuille | Installer le portefeuille 0.18.0 |
| Journal du nœud | — | `journalctl -u vinx-node -n 100` |

**Revenir en arrière** : l'ancienne chaîne est dans
`/var/backups/vinx/chaine-AAAAMMJJ-HHMMSS.tar.gz` sur chaque nœud. Pour la
restaurer, sur chaque nœud :

1. `systemctl stop vinx-node` ;
2. décompresser l'archive à la place de `/var/lib/vinx` et de
   `/etc/vinx/genesis.json` ;
3. remettre le binaire 0.8.1 ;
4. redémarrer le nœud.
