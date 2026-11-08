# Lancer la pré-production (protocoles 7 et 8, modèle réserve)

La pré-production est une nouvelle chaîne, `vinx-preprod-1`, **en conditions
réelles** : modèle réserve (protocole 8), aucune durée raccourcie, pas de
robinet, **aucune part réservée**. Elle démarre avec **un seul validateur**
(Cinnamon). Les trois autres entrent ensuite, financés par l'émission que
Cinnamon a reçue. Les règles : [PROTOCOLE-7.md](./PROTOCOLE-7.md) et
[PROTOCOLE-8.md](./PROTOCOLE-8.md).

Compter environ **4 jours** avant d'avoir les 4 validateurs : 1 jour pour la
première époque, puis 48 h de délai admin pour les ajouts.

| Machine | Adresse | Rôle |
|---|---|---|
| Cinnamon | 192.168.1.22 | validateur dès la genèse |
| Amber | 192.168.1.21 | suit la chaîne, validateur au jour 3–4 |
| Obsidian | 192.168.1.23 | idem |
| Pearl | 192.168.1.233 | idem |
| Copper (`vinx-public`) | 192.168.1.244 | nœud public (archive) |
| Ton PC Windows | | clés admin, cockpit, genèse |

> **À décider avant de commencer** : ce guide remplace le testnet 2 sur les
> mêmes machines (`reset-chain` en garde une sauvegarde dans
> `/var/backups/vinx/`). Pour garder le testnet 2 en parallèle, il faut
> d'autres machines ou d'autres ports : dis-le-moi et j'adapte le guide.

## 1. Publier la release

Après la fusion de la PR des protocoles 7 et 8 dans `main` : GitHub,
**Releases**, **Draft a new release**, tag `vX.Y.Z` sur `main` (la version
de `Cargo.toml`). Attends les archives (environ 15 minutes).

Sur ton PC, décompresse l'archive Windows à la place de l'ancienne version et
vérifie :

```
vinx-node --version
vinx-wallet --version
```

## 2. Écrire la genèse (sur ton PC)

Il faut les clés publiques des 3 admins et **de Cinnamon seulement**.

- Admins, dans `C:\Users\Administrateur\vinx-cockpit` :

  ```
  vinx-wallet address --key admin-1.json
  vinx-wallet address --key admin-2.json
  vinx-wallet address --key admin-3.json
  ```

- Cinnamon, depuis `vinx-public` :

  ```bash
  ssh root@192.168.1.22 cat /etc/vinx/validator.pub
  ```

Puis, sur une seule ligne sous Windows (remplace les `\` par des espaces) :

```
vinx-node genesis --chain-id vinx-preprod-1 --model reserve \
  --admin CLE-ADMIN-1 --admin CLE-ADMIN-2 --admin CLE-ADMIN-3 \
  --validator CLE-CINNAMON --identity "Cinnamon,VinX Labs,FR" \
  --required-bond 1000 \
  --out genesis.json
```

Vérifie dans `genesis.json` :

- `"chain_id": "vinx-preprod-1"`, `"protocol_version": 8`, `"model": "reserve"` ;
- `"treasury_balance": "0"` et `"tranches": []` (pas d'escrow) ;
- un seul validateur, `"validator_identities"` avec Cinnamon ;
- `"required_bond": "1000000000000"` (1 000 VINX).

## 3. Démarrer Cinnamon, puis les autres nœuds

Copie la genèse et l'archive Linux sur les 5 serveurs, en passant par
`vinx-public` :

```
scp genesis.json vinx-X.Y.Z-linux-x86_64.tar.gz root@192.168.1.244:/root/
```

```bash
cd /root && tar xzf vinx-X.Y.Z-linux-x86_64.tar.gz
for ip in 22 21 23 233; do scp /root/genesis.json /root/vinx-X.Y.Z-linux-x86_64.tar.gz root@192.168.1.$ip:/root/; done
```

**Cinnamon d'abord** : seul validateur, il produit les blocs dès son
démarrage.

```bash
ssh root@192.168.1.22
cd /root && tar xzf vinx-X.Y.Z-linux-x86_64.tar.gz
cd vinx-X.Y.Z-linux-x86_64 && ./install-validator.sh reset-chain /root/genesis.json
```

Vérifie que la hauteur avance :

```bash
curl -s http://127.0.0.1:8545/status
```

Puis **Amber, Obsidian et Pearl** (même commande sur chacun) : ils suivent la
chaîne sans voter, avec leur clé de validateur déjà prête. Enfin **Copper**
(`vinx-public`), en dernier.

## 4. Jour 1 : la première époque

Une époque dure 17 280 blocs, environ 24 h. À sa fin, Cinnamon reçoit toute
l'émission du jour (environ 19 000 VINX). Vérifie, sur Cinnamon :

```bash
/root/vinx-X.Y.Z-linux-x86_64/vinx-wallet balance $(/root/vinx-X.Y.Z-linux-x86_64/vinx-wallet address --key /etc/vinx/validator.json | awk '/adresse/ {print $2}')
```

## 5. Jour 1 : cautions et financement des trois autres

Sur **Cinnamon**, dépose sa caution :

```bash
cd /root/vinx-X.Y.Z-linux-x86_64
./vinx-wallet --password-file /etc/vinx/key-password validator bond --key /etc/vinx/validator.json --amount 1000
```

Sur **chacun des trois autres**, note son adresse :

```bash
/root/vinx-X.Y.Z-linux-x86_64/vinx-wallet address --key /etc/vinx/validator.json
```

Sur **Cinnamon**, envoie 1 100 VINX à chacun (caution et frais) :

```bash
./vinx-wallet --password-file /etc/vinx/key-password send --key /etc/vinx/validator.json --to ADRESSE-AMBER --amount 1100
./vinx-wallet --password-file /etc/vinx/key-password send --key /etc/vinx/validator.json --to ADRESSE-OBSIDIAN --amount 1100
./vinx-wallet --password-file /etc/vinx/key-password send --key /etc/vinx/validator.json --to ADRESSE-PEARL --amount 1100
```

Sur **chacun des trois autres**, dépose la caution :

```bash
cd /root/vinx-X.Y.Z-linux-x86_64
./vinx-wallet --password-file /etc/vinx/key-password validator bond --key /etc/vinx/validator.json --amount 1000
```

## 6. Jour 1 : proposer les trois ajouts (cockpit)

Sur ton PC, ouvre le tunnel vers un nœud puis le cockpit, comme d'habitude.
Onglet **Actions**, carte **Ajouter un validateur**, pour chacun :

- clé publique (`cat /etc/vinx/validator.pub` sur le serveur) ;
- nom (Amber, Obsidian, Pearl), opérateur « VinX Labs », pays « FR ».

Signe chaque proposition avec deux clés admin et envoie les **trois le même
jour** : elles s'exécutent ensemble 48 h plus tard, et l'ensemble passe
directement de 1 à 4 validateurs.

> **Important** : avant l'échéance, vérifie que les trois nœuds tournent et
> sont à jour (même hauteur que Cinnamon). À 4 validateurs, il en faut 3 pour
> produire un bloc : deux nœuds en retard arrêteraient la chaîne.

Pendant les 48 h, les ajouts sont visibles et annulables dans **Gouvernance,
Actions admin en attente**.

## 7. Jour 3–4 : vérifier

- Explorateur, onglet **Validateurs** : 4 fiches actives, cautions de
  1 000 VINX, présence proche de 100 %.
- Onglet **Réserve** : 900 000 000 VINX, aucune demande.
- Tableau de bord : 4 validateurs qui votent.

## 8. Essais à faire ensuite

1. **Demande à la réserve** : depuis le portefeuille de bureau (Plus,
   Réserve) ou `vinx-wallet reserve request`. Les validateurs votent :

   ```bash
   ./vinx-wallet --password-file /etc/vinx/key-password validator vote --key /etc/vinx/validator.json --proposal IDENTIFIANT
   ```

   Puis vote des détenteurs pendant 14 jours (portefeuille, onglet Réserve).
   Le quorum minimal est de 100 000 VINX : au début, seuls les validateurs
   en détiennent autant.
2. **Paramètre voté** : cockpit, Actions, Paramètres du réseau. Au modèle
   réserve, le bouton crée une proposition soumise au vote.
3. **Présence et mise à l'écart** : arrêter un validateur plus de 1 000 blocs
   (environ 1 h 25), puis le faire revenir avec `validator unjail` après
   1 000 blocs de plus.
4. **Omissions** : la colonne Omissions de l'onglet Validateurs doit rester
   faible pour tous.

## En cas de problème

| Symptôme | Cause probable | Que faire |
|---|---|---|
| `reset-chain` refuse la genèse | Ancien script, sans « preprod » | Utiliser le script de la nouvelle archive |
| La hauteur reste à 0 | Cinnamon n'est pas démarré, ou n'a pas la bonne genèse | `journalctl -u vinx-node -n 100` sur Cinnamon |
| Pas d'émission après 24 h | Présence de Cinnamon sous 97 % | Onglet Validateurs ; journal du nœud |
| `validator bond` : solde insuffisant | Première époque pas encore close | Attendre la fin de l'époque |
| La chaîne s'arrête à l'entrée des 3 validateurs | Nœuds pas à jour | Les redémarrer ; ils rattrapent la chaîne, qui repart dès que 3 sur 4 votent |
