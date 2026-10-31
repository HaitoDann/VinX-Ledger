# Exercices de panne

Quatre exercices qui vérifient, sur un vrai réseau, que VinX Ledger se comporte
comme prévu quand des validateurs tombent. Ils se rejouent après chaque
changement important (nouvelle version, nouveau validateur, nouvel hébergement)
et devraient être faits par tout nouvel opérateur.

Prérequis :

- un réseau de **4 validateurs** et une machine d'où les joindre en SSH (ici le
  nœud public) ;
- une clé SSH vers chaque validateur (`ssh-copy-id`), pour ne pas taper de mot
  de passe à chaque commande ;
- le tableau de bord ouvert pendant les exercices.

**Lance les commandes une par une** : plusieurs étapes demandent d'attendre
avant de mesurer.

Ci-dessous, `V1` à `V4` désignent les adresses des validateurs, et
`hauteurs` la commande :

```bash
for ip in V1 V2 V3 V4; do printf "$ip  "; ssh root@$ip "curl -s 127.0.0.1:8545/status" | grep -oE '"height":[0-9]+'; done
```

## 1. Un validateur en panne : la chaîne continue

Avec 4 validateurs, il faut plus des deux tiers des votes : 3 suffisent.

```bash
hauteurs                                          # tous égaux
ssh root@V4 "systemctl stop vinx-node"; date
# attendre 1 à 2 minutes, mesurer deux fois : la hauteur avance
curl -s 127.0.0.1:8545/status | grep -oE '"height":[0-9]+'
ssh root@V4 "systemctl start vinx-node"; sleep 60
hauteurs                                          # V4 a rattrapé
ssh root@V4 "journalctl -u vinx-node --since '-10min' --no-pager | grep -iE 'error|panic' | tail"
```

Attendu : la chaîne avance pendant la panne ; un bloc sur quatre (le tour de
V4) passe au round 1 après un court délai ; V4 rattrape seul à son retour.

## 2. Deux validateurs en panne : arrêt sûr, puis reprise

Avec 2 validateurs sur 4, il n'y a plus de majorité : la chaîne doit
**s'arrêter**, sans bloc incohérent, et repartir seule.

```bash
ssh root@V4 "systemctl stop vinx-node"; ssh root@V3 "systemctl stop vinx-node"; date
# attendre 2 minutes, puis mesurer deux fois à 1 minute d'écart :
curl -s 127.0.0.1:8545/status | grep -oE '"height":[0-9]+'   # identiques
ssh root@V3 "systemctl start vinx-node"; ssh root@V4 "systemctl start vinx-node"
# après 1 minute : la hauteur repart, puis
hauteurs
for ip in V1 V2 V3 V4; do ssh root@$ip "journalctl -u vinx-node --since '-15min' --no-pager | grep -iE 'error|panic|équivoque' | tail -5"; done
```

Attendu : hauteur figée pendant la panne ; reprise automatique ; aucune
équivoque. Un paiement envoyé pendant la panne reste en attente et passe à la
reprise.

## 3. Perte d'un disque : reconstruction par instantané

On efface les données d'un validateur en gardant son identité réseau
(`network.key`) ; sa clé de validateur, dans `/etc/vinx`, n'est pas touchée.
La mémoire anti-double vote est effacée avec la base : attendre **au moins une
minute** avant de redémarrer, pour que la chaîne ait dépassé les hauteurs où
il a voté.

```bash
ssh root@V2 "systemctl stop vinx-node; date"
ssh root@V2 "mv /var/lib/vinx /var/lib/vinx.sauvegarde && mkdir /var/lib/vinx && cp -p /var/lib/vinx.sauvegarde/network.key /var/lib/vinx/ && chown --reference=/var/lib/vinx.sauvegarde /var/lib/vinx"
# attendre 1 minute
ssh root@V2 "systemctl start vinx-node"
# après 2 minutes
hauteurs
ssh root@V2 "journalctl -u vinx-node --since '-5min' --no-pager | grep -iE 'instantan|synchro|error|panic' | tail"
```

Attendu : « synchronisation par l'état », « état installé depuis un
instantané », puis V2 à la hauteur des autres. Retour arrière si besoin :
arrêter le nœud et remettre `/var/lib/vinx.sauvegarde` en place.

## 4. Coupure de courant : arrêt brutal répété

Le processus est tué sans pouvoir fermer sa base (`kill -9`), trois fois.

```bash
for i in 1 2 3; do
  ssh root@V3 "systemctl kill -s KILL vinx-node; date"; sleep 20
  ssh root@V3 "systemctl start vinx-node"; sleep 40
done
hauteurs
ssh root@V3 "journalctl -u vinx-node --since '-5min' --no-pager | grep -iE 'error|panic|corrupt|répar' | tail"
for ip in V1 V2 V4; do ssh root@$ip "journalctl -u vinx-node --since '-5min' --no-pager | grep -iE 'équivoque|preuve' | tail"; done
```

Attendu : redémarrage et rattrapage à chaque fois ; aucune corruption ; aucune
équivoque chez les autres validateurs (l'onglet Gouvernance du tableau de bord
liste les preuves d'équivoque : il reste vide).

## Résultats

### Testnet 2, version 0.20.1 — 30 et 31 octobre 2026

4 validateurs et un nœud public, sur un même serveur Proxmox, en réseau local.

| Exercice | Résultat |
|---|---|
| 1. Un validateur arrêté | ✅ La chaîne a continué (14 798 → 14 805 pendant la panne). V4 a rattrapé en moins d'une minute (4 nœuds à 14 826–14 829). Journal sans erreur. |
| 2. Deux validateurs arrêtés | ✅ Hauteur figée à 19 468 pendant 45 minutes (mesures à 00:30:08 et 00:31:24). Reprise automatique, 4 validateurs en ligne à 19 477. Aucune équivoque. |
| 3. Disque effacé | ✅ Instantané de la hauteur 19 600 installé moins d'une seconde après le démarrage ; 4 nœuds à 19 695 deux minutes plus tard. Base de 680 Ko au lieu de 16 Mo. |
| 4. Trois arrêts brutaux | ✅ Les 4 nœuds à 19 780 ; ni erreur, ni corruption, ni équivoque. |

Limite : toutes les machines partagent un serveur et un réseau local. Ces
résultats ne disent rien de la latence entre sites distants ni d'une coupure
d'Internet entre opérateurs : à refaire quand les validateurs seront répartis.

## Rejoués automatiquement

Depuis le lot S2, les exercices 1 à 4 sont aussi rejoués par la CI à chaque
modification du code, sur quatre validateurs locaux
(`crates/vinx-node/tests/network.rs` et `sync.rs`) : une régression qui
empêcherait la chaîne de survivre à ces pannes bloque la fusion. Les refaire
sur le vrai réseau reste utile après un changement d'infrastructure.
