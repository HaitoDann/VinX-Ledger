# Nœud public

Un nœud public suit la chaîne sans clé : il ne vote pas, ne peut rien signer et
ne détient aucun fonds. Il sert l'API et le tableau de bord en HTTPS, pour que
le portefeuille et le cockpit fonctionnent sans tunnel SSH.

```
Portefeuille ──HTTPS──▶ Caddy ──▶ nœud public (observateur) ──P2P──▶ validateurs
```

Ce guide l'installe dans un conteneur LXC Proxmox, derrière une box internet.
Remplace les mots en MAJUSCULES par tes valeurs.

## 0. Ce qu'il faut

- La release 0.8.1 ou plus.
- La genèse du réseau (`genesis.json`) et l'adresse réseau d'au moins un
  validateur (`/ip4/…/tcp/26656/p2p/12D3KooW…`, affichée par `prepare` sur le validateur).
- Un nom de domaine qui pointe vers l'IP publique de ta box :
  - un domaine à toi : un enregistrement `A` vers ton IP publique ;
  - ou gratuit : un sous-domaine DuckDNS (`ton-nom.duckdns.org`), qui suit aussi
    une IP qui change.

## 1. Créer le conteneur

Dans l'interface Proxmox, **Create CT** :

| Onglet | Valeur |
|---|---|
| General | Hostname `vinx-public`, conteneur **unprivileged**, option **Nesting** cochée |
| Template | `debian-12-standard` |
| Disks | 20 Go |
| CPU | 1 cœur |
| Memory | 2048 Mo, swap 512 Mo |
| Network | `vmbr0`, IPv4 statique, par exemple `192.168.1.50/24`, passerelle de la box |

Démarre-le, puis ouvre sa **Console** :

```bash
apt update && apt upgrade -y && apt install -y curl ca-certificates caddy
```

## 2. Installer le nœud en observateur

Télécharge et décompresse la release Linux dans le conteneur :

```bash
cd /root
curl -LO https://github.com/HaitoDann/VinX-Ledger/releases/download/vVERSION/vinx-VERSION-linux-x86_64.tar.gz
tar xzf vinx-VERSION-linux-x86_64.tar.gz && cd vinx-VERSION-linux-x86_64
./install-validator.sh prepare --observer
```

Le script affiche l'**identifiant réseau** du nœud public (`12D3KooW…`) : note-le.

Copie la genèse du réseau dans `/root/genesis.json`, depuis un validateur :

```bash
scp root@IP-DU-VALIDATEUR:/etc/vinx/genesis.json /root/genesis.json
```

## 3. Autoriser le nœud public sur les validateurs

Un validateur refuse les pairs inconnus. Sur **chaque** validateur, un par un :

```bash
cat /etc/vinx/vinx-node.env
```

La ligne `VINX_OPTIONS="…"` donne les options actuelles. Relance `start` avec
**les mêmes options**, plus `--allow` :

```bash
cd /root/vinx-VERSION-linux-x86_64
./install-validator.sh start /etc/vinx/genesis.json OPTIONS-ACTUELLES --allow ID-DU-NOEUD-PUBLIC
```

`start` remplace toutes les options : sans les `--peer` actuels, le validateur
perdrait ses voisins. Attends qu'il ait repris la chaîne (hauteur qui monte au
tableau de bord) avant de passer au suivant.

## 4. Démarrer le nœud public

Dans le conteneur, avec l'adresse réseau des validateurs (au moins deux, si possible) :

```bash
./install-validator.sh start /root/genesis.json --node-name Copper \
  --peer /ip4/IP-VALIDATEUR-1/tcp/26656/p2p/ID-VALIDATEUR-1 \
  --peer /ip4/IP-VALIDATEUR-2/tcp/26656/p2p/ID-VALIDATEUR-2
```

`--node-name Copper` affiche le nom du nœud public dans l'explorateur et le
tableau de bord (« Copper · vinx-testnet-2 »).

Le script attend que le nœud réponde et affiche son statut. Vérifie que la hauteur
rattrape celle des validateurs :

```bash
curl -s 127.0.0.1:8545/status | grep -o '"height":[0-9]*'
```

Le nœud public se connecte vers les validateurs : il n'a pas besoin du port 26656
en entrée.

## 5. Ouvrir le HTTPS

1. **Sur la box** : redirige les ports TCP **80** et **443** vers l'IP du
   conteneur (`192.168.1.50`). Le menu s'appelle selon la box « NAT/PAT »,
   « Redirection de ports » ou « Port forwarding ». Le port 80 sert à obtenir
   le certificat.
2. **Dans le conteneur** :

   ```bash
   cp public/Caddyfile /etc/caddy/Caddyfile
   sed -i 's/NOEUD.EXEMPLE.FR/TON-DOMAINE/' /etc/caddy/Caddyfile
   systemctl reload caddy
   ```

3. Caddy obtient le certificat tout seul en quelques secondes. Ouvre
   `https://TON-DOMAINE/` depuis ton téléphone, en 4G et hors du Wi-Fi pour
   tester l'accès depuis l'extérieur : le tableau de bord s'affiche.

En cas de problème : `journalctl -u caddy -n 50`. La cause la plus fréquente est
un domaine qui ne pointe pas encore vers ta box, ou un port 80 fermé.

## 6. Brancher le portefeuille et le cockpit

- **Portefeuille** → Réglages → Adresse du nœud : `https://TON-DOMAINE`.
- **Cockpit** → onglet Réseau : ajoute `https://TON-DOMAINE`.

Les envois passent par le nœud public, qui les relaie aux validateurs. Ni les
clés ni les mots de passe ne transitent : seules des transactions déjà signées
quittent ton PC.

## 7. Mettre à jour

Comme un validateur : `./install-validator.sh upgrade` depuis le dossier de la
nouvelle release. Le script sauvegarde, redémarre et revient en arrière si le
nœud ne suit plus la chaîne.

## Ce qui est exposé

| Exposé | Pas exposé |
|---|---|
| Le tableau de bord, l'explorateur (`/explorer`), l'API en lecture (comptes, blocs, noms, séquestres, jetons) | Les métriques `/metrics` (bloquées par Caddy) |
| L'envoi de transactions signées (`POST /transactions`) | Aucune clé : le nœud n'en a pas |
| | Le port P2P : uniquement des connexions sortantes |

Protections du nœud :

- au plus 64 requêtes traitées en même temps ;
- au plus **600 requêtes par minute et par visiteur** (`--rate-limit`, 0 pour
  désactiver) ; au-delà, le nœud répond 429 « trop de requêtes ». Derrière
  Caddy, l'adresse du visiteur est lue dans l'en-tête `X-Forwarded-For` que
  Caddy ajoute ; un visiteur ne peut pas la falsifier ;
- les routes qui parcourent l'état (comptes, jetons, noms…) travaillent sur une
  copie de l'état et ne bloquent pas le consensus ;
- la page d'accueil de l'explorateur fait 7 requêtes par rafraîchissement,
  dont `/transactions/recent`, calculée une fois par bloc.
