# VinX — Testnet officiel

La chaîne de test publique porte l'identifiant **7**. Ses VINX n'ont aucune valeur. Elle
applique les règles du mainnet :
- garantie minimale de **1 000 VINX** ;
- récompenses proportionnelles à l'enjeu et à la présence ;
- entrée libre des validateurs.

Le faucet donne **1 100 VINX par adresse et par jour**, de quoi devenir validateur en une
seule demande.

Deux rôles :
- **l'opérateur** lance la genèse et un premier point d'entrée ;
- **tout le monde** peut ensuite rejoindre le réseau, avec l'application ou en ligne de
  commande.

---

## 1. Opérateur : lancement

### Serveur

- une IP publique fixe et, de préférence, un nom DNS (par exemple `seed.vinx.example`) ;
- 2 vCPU, 2 Go de RAM, 20 Go de disque, sous Debian 12 ou Ubuntu 22.04 et plus récent ;
- les ports **8545/TCP** (API, interface web, genèse, faucet) et **9001/TCP+UDP** (P2P)
  ouverts.

### Étapes

```bash
apt install -y build-essential git curl python3
curl https://sh.rustup.rs -sSf | sh -s -- -y
git clone https://github.com/HaitoDann/vinx-ledger && cd vinx-ledger

./vinx start --testnet           # genèse de la chaîne 7, 1 validateur, faucet
sudo ./vinx service              # redémarrage automatique et au démarrage
./vinx alert                     # alertes ntfy sur le téléphone
./vinx publish seed.vinx.example # genèse officielle + point d'entrée dans le dépôt
git add genesis-testnet.json seeds/testnet.txt && git commit -m "testnet: genèse officielle" && git push
git tag v0.2.0 && git push --tags   # Release GitHub : binaires et installateurs
```

`./vinx publish` vérifie que le serveur est joignable de l'extérieur. Il copie ensuite la
genèse dans `genesis-testnet.json` et ajoute le serveur à `seeds/testnet.txt`. Le logiciel
et l'application embarquent ces deux fichiers. `./vinx join` refuse une genèse différente
de la genèse publiée : un faux point d'entrée ne peut pas faire rejoindre une autre chaîne.

Le tag `v…` déclenche la construction des binaires et des installateurs (Windows, macOS,
Linux) et les attache à la **Release GitHub**, avec des liens directs sans compte.

**Décentralisation :** au départ, le serveur est le seul validateur. Faites entrer d'autres
validateurs, sur d'autres machines et chez d'autres personnes, avant d'annoncer le réseau.
Tant qu'une seule machine détient plus d'un tiers de la puissance de vote, la chaîne
dépend d'elle.

---

## 2. Rejoindre le testnet

### Avec l'application (le plus simple)

1. Téléchargez VinX depuis la page **Releases** du dépôt GitHub, puis installez-le.
2. Créez un portefeuille : notez les 12 mots, puis choisissez un mot de passe.
3. **Accueil → VINX de test** : 1 100 VINX arrivent au bloc suivant.
4. **Valider** : activez l'interrupteur. Le validateur démarre, dépose 1 000 VINX de
   garantie, fait son échauffement (environ 3 h) et entre dans le set.

### En ligne de commande (serveur, Raspberry Pi, LXC)

```bash
git clone https://github.com/HaitoDann/vinx-ledger && cd vinx-ledger
./vinx join            # point d'entrée officiel, faucet, garantie, échauffement
sudo ./vinx service    # le nœud survit aux redémarrages
./vinx alert           # alertes ntfy sur le téléphone
./vinx status
```

### Bon à savoir

- **Ports** : le 9001 est facultatif. Le nœud essaie de l'ouvrir lui-même sur la box
  (UPnP). Sinon, il passe par deux relais et tente de percer la box pour une liaison
  directe. L'ouvrir rend le réseau plus solide.
- **Point d'entrée à la maison** : un nœud lancé avec `--testnet` ou `--lan` demande aussi
  à la box d'ouvrir le port 8545 (API : genèse, faucet). Si la box accepte l'UPnP,
  `./vinx status` affiche « API ouverte sur la box » et l'adresse à donner aux nouveaux
  venus (`./vinx join <adresse>`). Sinon, rediriger 8545/TCP et 9001/TCP à la main.
- **Absence** : un validateur qui rate ses blocs est suspendu et sort du vote
  immédiatement, sans perte d'argent. Une fois de retour en ligne, il se réhabilite tout
  seul.
- **Double vote** : il est sanctionné. Ne lancez jamais deux fois le même validateur.
- **Récompenses** : environ 26 VINX sont émis par bloc au départ (en baisse continue). 20 % vont au proposeur, et le
  reste est partagé à chaque époque selon l'enjeu et la présence.

---

## 3. Mises à jour

Plus de redémarrage depuis la genèse (ADR 0086) :
1. une nouvelle version est publiée (Release GitHub) ;
2. elle est annoncée sur la chaîne, avec un préavis ;
3. elle s'active quand plus des 2/3 des validateurs l'ont installée.

Un nœud resté sur une ancienne version s'arrête proprement et l'indique dans
`./vinx status` et par alerte. Il suffit de faire `git pull` puis de le relancer, ou de
réinstaller l'application.

---

## 4. Tests à mener ensemble

| Test | Comment | Attendu |
|---|---|---|
| Charge | `./vinx load 1000` | 1 000 transferts inclus en quelques blocs |
| Panne | arrêter un validateur | la chaîne continue, il est suspendu puis revient seul |
| Pannes successives | en arrêter plusieurs, l'un après l'autre | la chaîne continue tant que chaque vague reste sous le tiers |
| Perte de quorum | arrêter plus d'un tiers d'un coup | arrêt sans fork, reprise au retour |
| Paiement et reçu | application → Envoyer, puis reçu | reçu vérifiable hors ligne |
| Box | validateur sans port ouvert | `./vinx status` : « via relais », il valide quand même |

Signalez tout comportement anormal en joignant le fichier produit par `./vinx diag`.

---

## 5. Limites connues

- L'annonce des mises à jour passe par la clé d'administration, qui expire au bout de
  365 jours. La remplacer par un vote des validateurs est prévu avant le mainnet.
- Les installateurs ne sont pas signés : Windows et macOS affichent un avertissement.
- L'audit de sécurité externe reste à faire avant le mainnet.
