# VinX — Testnet public

Ce guide explique comment lancer la chaîne de test publique (chaîne **7**). Il y a deux
rôles : **l'opérateur**, qui héberge le nœud d'amorçage, et **les validateurs**, qui le
rejoignent. Les VINX du testnet n'ont aucune valeur.

---

## 1. Opérateur : le nœud d'amorçage

### Serveur

Le serveur doit avoir les caractéristiques suivantes :
- une IP publique fixe (VPS ou box avec redirection de ports) et un nom DNS conseillé,
  par exemple `seed.vinx.example` ;
- 2 vCPU, 2 Go de RAM, 20 Go de disque ;
- Debian 12 ou Ubuntu 22.04+.

Les ports à ouvrir :

| Port | Protocole | Rôle |
|---|---|---|
| 8545 | TCP | API, interface web, genèse et synchronisation |
| 9001 | TCP + UDP | P2P (consensus, transactions) |

### Lancement

```bash
apt install -y build-essential git curl python3
curl https://sh.rustup.rs -sSf | sh -s -- -y
git clone https://github.com/HaitoDann/vinx-ledger && cd vinx-ledger

./vinx start --testnet -n 3        # chaîne 7, 3 validateurs sur le serveur
sudo ./vinx service                # services systemd : redémarrage automatique et au boot
./vinx status
```

`--testnet` fait trois choses :
- la chaîne prend l'identifiant 7 ;
- le nœud écoute sur toutes les interfaces ;
- le faucet donne 100 VINX par adresse et par jour.

Le temps de bloc est de 12 s, c'est le paramètre du protocole. Lancer 3 validateurs sur
le serveur permet ensuite de tolérer une panne dès qu'un validateur extérieur arrive
(4 au total).

L'interface publique (portefeuille, explorateur et page **Réseau**) est servie sur
`http://<serveur>:8545/`. Pour la proposer en HTTPS, placez-la derrière un proxy
(le `Caddyfile` fourni sert d'exemple).

### Entrée des validateurs : libre

Personne n'a besoin de l'opérateur pour devenir validateur. `./vinx join` demande des
VINX de test au faucet, dépose la garantie minimale (50 VINX sur le testnet, 10 000 sur
le mainnet) avec la clé de vote BLS, puis le nœud fait son **échauffement** : 3 fins
d'époque (environ 3 h) avant d'entrer dans le set. L'échauffement empêche d'entrer et
sortir en boucle pour perturber la chaîne.

`./vinx accept vinx1…` existe encore pour l'opérateur : il fait entrer un validateur
immédiatement, sans échauffement (pratique pour un test). Ce n'est plus nécessaire.

### Sauvegarde

Le dossier `.vinx-local/` contient tout l'état du nœud. Les clés qu'il contient sont
secrètes : `node*/validator.json`, `node*/validator_bls.json` et `node1/admin.json`.
Sauvegardez-les à part.

---

## 2. Validateurs : rejoindre le testnet

```bash
apt install -y build-essential git curl python3
curl https://sh.rustup.rs -sSf | sh -s -- -y
git clone https://github.com/HaitoDann/vinx-ledger && cd vinx-ledger

./vinx join                        # essaie les points d'entrée de seeds/testnet.txt
./vinx join seed.vinx.example      # ou un (ou plusieurs) point(s) d'entrée précis
```

Le nœud n'a besoin du point d'entrée que pour arriver. Ensuite il découvre les autres
nœuds (Kademlia), s'y connecte directement (jusqu'à 25 pairs) et les mémorise dans
`peers.json` : si le point d'entrée disparaît, il continue et se reconnecte au
redémarrage sans lui. `./vinx status` affiche le nombre de pairs.

Le nœud se synchronise, reçoit des VINX de test du faucet, dépose sa garantie et entre
dans le set après son échauffement (environ 3 h), sans intervention de l'opérateur.

Ensuite :
```bash
sudo ./vinx service                # le nœud tourne en service et survit aux redémarrages
./vinx status
```

Ouvrez le port **9001 TCP/UDP**. Restez en ligne : un validateur absent manque ses tours
de proposition et finit écarté (jailing, sans perte d'argent). Le double vote, lui, est
sanctionné : ne lancez jamais deux fois le même validateur.

---

### Ajouter un point d'entrée

Tout nœud joignable (port 9001 ouvert, IP ou nom DNS stable) peut servir de point
d'entrée. Ajoutez une ligne à `seeds/testnet.txt` (le binaire l'embarque à la
compilation). Plusieurs points d'entrée tenus par des personnes différentes : le
réseau ne dépend plus d'aucun d'eux.

---

## 3. Tests à mener ensemble

| Test | Commande | Attendu |
|---|---|---|
| Charge | `./vinx load 1000` (opérateur) | 1 000 transferts inclus en 1 ou 2 blocs |
| Panne | arrêter un validateur | la chaîne continue tant que plus des 2/3 sont en ligne |
| Perte de quorum | arrêter plus d'1/3 | la chaîne s'arrête sans fork, puis repart quand ils reviennent |
| Paiement et reçu | interface → Envoyer → Télécharger le reçu | reçu vérifiable hors ligne (`vinx-wallet verify-receipt`) |
| Redémarrage | `systemctl restart vinx-node1` | le nœud rattrape la chaîne |

Remontez tout comportement anormal en joignant la sortie de `./vinx status` et de
`journalctl -u vinx-node1 -n 200`.

---

## 4. Limites connues du testnet

- **Les clés de test sont stockées en clair** dans `.vinx-local/`. Il ne faut jamais y
  mettre de valeur réelle.
- **Pas de checkpoints configurés.** Un nouveau nœud fait confiance à la genèse que lui
  sert l'hôte (ADR 0074). Pour le mainnet, la genèse et les checkpoints seront publiés
  avec le binaire.
- **L'audit de sécurité externe reste à faire** avant le mainnet.
