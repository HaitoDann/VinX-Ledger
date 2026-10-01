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

### Admettre des validateurs

```bash
./vinx pending                     # demandes envoyées par ./vinx join
./vinx accept vinx1…               # envoie 10 001 VINX de test, attend le dépôt, ajoute au set
```

Pendant le testnet, l'admission passe par l'opérateur (action d'administration). Cette
clé d'administration s'éteint d'elle-même au bout de 365 jours (ADR 0081). Sur le
mainnet, il suffira de bonder.

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

./vinx join seed.vinx.example      # récupère la genèse, synchronise, envoie la demande
```

Le nœud se synchronise, puis attend. Prévenez l'opérateur (Discord, etc.) : quand il
lance `./vinx accept`, votre nœud reçoit les fonds de test, dépose sa garantie avec sa
clé de vote BLS et entre dans le set automatiquement.

Ensuite :
```bash
sudo ./vinx service                # le nœud tourne en service et survit aux redémarrages
./vinx status
```

Ouvrez le port **9001 TCP/UDP**. Restez en ligne : un validateur absent manque ses tours
de proposition et finit écarté (jailing, sans perte d'argent). Le double vote, lui, est
sanctionné : ne lancez jamais deux fois le même validateur.

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

- **L'admission des validateurs passe par l'opérateur.** C'est volontaire pour le
  testnet.
- **Les clés de test sont stockées en clair** dans `.vinx-local/`. Il ne faut jamais y
  mettre de valeur réelle.
- **Pas de checkpoints configurés.** Un nouveau nœud fait confiance à la genèse que lui
  sert l'hôte (ADR 0074). Pour le mainnet, la genèse et les checkpoints seront publiés
  avec le binaire.
- **L'audit de sécurité externe reste à faire** avant le mainnet.
