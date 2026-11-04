# Lancer le testnet 3 (VinX 0.29, protocole 7)

Ce guide démarre une **nouvelle chaîne**, `vinx-testnet-3`, directement au
**protocole 7** : validateurs nommés, cautions, présence mesurée à chaque bloc,
récompenses réservées aux validateurs disponibles, mise à l'écart, départ et
sanction de la double signature (voir [PROTOCOLE-7.md](./PROTOCOLE-7.md)).

Le testnet 2 n'est pas modifié : on peut le laisser tourner à côté le temps
des essais, sur d'autres machines ou d'autres ports, ou l'arrêter.

| Machine | Adresse | Rôle |
|---|---|---|
| Cinnamon | 192.168.1.22 | validateur 1 |
| Amber | 192.168.1.21 | validateur 2 |
| Obsidian | 192.168.1.23 | validateur 3 |
| Pearl | 192.168.1.233 | validateur 4 |
| Copper (`vinx-public`) | 192.168.1.244 | nœud public (archive) |
| Ton poste | | clés admin, cockpit |

## 0. Ce qui change par rapport au testnet 2

- **Fiches dans la genèse** : chaque validateur a un nom public définitif
  (Cinnamon, Amber…), un opérateur et un pays. Les noms ne viennent plus de
  `/etc/vinx/validator-names.json`.
- **Cautions dans la genèse** : 10 000 VINX par validateur, pris sur le
  robinet (donc 960 000 VINX restent dans la trésorerie avec un robinet de
  1 000 000).
- **Délai admin de 48 h** : toute action admin (transfert de trésorerie,
  paramètre, ajout de validateur…) s'exécute 48 h après son inclusion, et
  peut être annulée entre-temps depuis le cockpit (onglet Gouvernance).
  **Les premiers versements du robinet aux testeurs arrivent donc 48 h après
  leur envoi.** Les validateurs, eux, sont payés dès la première époque.
- **Récompenses** : seulement aux validateurs qui ont signé au moins 97 % des
  blocs de l'époque.

## 1. Publier la release 0.29.0

Après la fusion de la PR du protocole 7 dans `main` : **Releases → Draft a
new release**, tag `v0.29.0` sur `main`, titre `VinX Ledger 0.29.0`. Attends
les archives (environ 15 minutes).

## 2. Mettre à jour ton poste

Décompresse l'archive Windows à la place de l'ancienne version, puis vérifie :

```
vinx-node --version
vinx-wallet --version
```

Les deux doivent afficher `0.29.0`.

## 3. Écrire la genèse (sur ton poste)

Rassemble les clés publiques, comme pour le testnet 2 : `vinx-wallet address
--key admin-N.json` pour les admins, `cat /etc/vinx/validator.pub` sur chaque
validateur. L'ordre des `--validator` et des `--identity` doit être le même.

Sur une seule ligne (sous Windows, remplace les `\` par des espaces) :

```
vinx-node genesis --chain-id vinx-testnet-3 \
  --admin CLE-ADMIN-1 --admin CLE-ADMIN-2 --admin CLE-ADMIN-3 \
  --validator CLE-CINNAMON --validator CLE-AMBER --validator CLE-OBSIDIAN --validator CLE-PEARL \
  --identity "Cinnamon,VinX Labs,FR" --identity "Amber,VinX Labs,FR" \
  --identity "Obsidian,VinX Labs,FR" --identity "Pearl,VinX Labs,FR" \
  --epoch-blocks 720 --faucet 1000000 \
  --out genesis.json
```

Vérifie le fichier :

- `"chain_id": "vinx-testnet-3"` et `"protocol_version": 7` ;
- `"validator_identities"` : les 4 noms, dans l'ordre des validateurs ;
- `"validator_bond": "10000000000000"` (10 000 VINX en unités).

## 4. Copier et basculer les nœuds

Exactement comme au testnet 2 (§4 et §5 de [TESTNET-2.md](./TESTNET-2.md)),
avec l'archive 0.29.0 et `install-validator.sh reset-chain
/root/genesis.json` : les 4 validateurs d'abord, `vinx-public` en dernier.
La chaîne démarre dès que 3 validateurs sur 4 tournent.

## 5. Vérifier

- Explorateur, nouvel onglet **Validateurs** : les 4 fiches, statut
  « actif », caution 10 000 VINX, présence proche de 100 %.
- `/governance` : `protocol_version` 7, bloc `staking` avec les paramètres.
- Après le bloc 720 : chaque validateur a reçu sa part de l'époque.

## 6. Essais du protocole 7 (checklist)

Fais-les dans cet ordre ; note tout ce qui surprend.

1. **Présence** : arrête un validateur (`systemctl stop vinx-node`) quelques
   minutes. Sa présence baisse dans l'explorateur ; la chaîne continue.
   Redémarre-le.
2. **Mise à l'écart** : arrête un validateur plus de 1 000 blocs (environ
   1 h 25 à 5 s le bloc). Il passe « mis à l'écart » et sort de l'ensemble ;
   le quorum se recalcule sur 3. Redémarre-le, attends encore 1 000 blocs
   après sa mise à l'écart, puis, sur ce validateur :

   ```
   vinx-wallet --node http://127.0.0.1:8545 validator unjail --key /etc/vinx/validator.json
   ```

   Il revient deux blocs plus tard.
3. **Délai admin** : depuis le cockpit, propose un transfert de trésorerie,
   signe, envoie. Il apparaît dans **Gouvernance → Actions admin en
   attente**. Essaie **Annuler…** sur un second transfert.
4. **Fiche** : sur un validateur, renseigne son site et son contact :

   ```
   vinx-wallet --node http://127.0.0.1:8545 validator info --key /etc/vinx/validator.json --website vinxlabs.fr --contact contact@vinxlabs.fr
   ```

5. **Départ et caution** (sur un 5ᵉ nœud de test si possible, pour garder 4
   validateurs) : `validator leave`, puis `validator withdraw` après 14 jours.
6. **Double signature** : pas d'essai sur le testnet (il faudrait lancer deux
   nœuds avec la même clé, ce qui est précisément la faute). C'est couvert par
   les tests automatiques ; le nœud qui relève une preuve soumet la sanction
   de lui-même.

## En cas de problème

| Symptôme | Cause probable | Que faire |
|---|---|---|
| `genesis` refuse les identités | Nom en double (sans tenir compte des majuscules), pays pas en 2 lettres, nom hors 3–24 caractères | Corriger `--identity` |
| `trésorerie insuffisante pour les cautions` | Robinet trop petit | `--faucet` d'au moins 40 000 |
| Un validateur n'est jamais payé | Présence sous 97 % sur l'époque | Regarder sa présence dans l'explorateur, son journal (`journalctl -u vinx-node -n 100`) |
| Un transfert de trésorerie « n'arrive pas » | Délai admin de 48 h | Cockpit, Gouvernance, Actions admin en attente |
