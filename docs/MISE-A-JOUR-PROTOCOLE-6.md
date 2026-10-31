# Passer le testnet 2 au protocole 6 (VinX 0.21)

Première mise à jour des règles **sur un réseau qui tourne, sans remise à
zéro** : c'est la procédure qui servira sur le réseau principal. Aucun solde,
nom, jeton ni historique n'est perdu.

Le protocole 6 ajoute la **réserve par objet** (SPEC §4.6) : chaque jeton
accepté, mandat ou règle de compte immobilise 0,1 VINX sur le compte qui le
possède, rendus quand l'objet est supprimé.

Durée : 30 minutes de manipulations, puis 24 heures d'attente imposées par la
chaîne entre la programmation et l'activation.

## 1. Publier la release 0.21.0

GitHub → Releases → Draft a new release → tag `v0.21.0` sur `main` → Publish.
Attendre les archives (environ 15 minutes).

## 2. Installer la 0.21.0 sur les 5 nœuds, un par un

La 0.21 sait exécuter le protocole 6, mais **ne l'active pas** : tant que la
mise à jour n'est pas programmée, rien ne change. On peut donc l'installer
tranquillement, un nœud à la fois, la chaîne continuant pendant chaque
redémarrage (exercice de panne n° 1).

Sur chaque machine, **validateurs d'abord, `vinx-public` en dernier** :

```bash
cd /root
wget https://github.com/HaitoDann/VinX-Ledger/releases/download/v0.21.0/vinx-0.21.0-linux-x86_64.tar.gz
tar xzf vinx-0.21.0-linux-x86_64.tar.gz && cd vinx-0.21.0-linux-x86_64
./install-validator.sh upgrade
```

`upgrade` sauvegarde les données, installe le binaire, redémarre et vérifie
que le nœud finalise des blocs ; sinon il revient de lui-même à la version
précédente.

Ensuite, vérifier que **les 5 nœuds sont en 0.21.0** (tableau de bord, colonne
version, ou) :

```bash
for ip in 22 21 23 233; do printf "192.168.1.$ip  "; ssh root@192.168.1.$ip "curl -s 127.0.0.1:8545/status" | grep -oE '"node_version":"[^"]*"'; done
curl -s 127.0.0.1:8545/status | grep -oE '"node_version":"[^"]*"'
```

**Ne passe à l'étape 3 que si tous affichent 0.21.0.** Un nœud resté en 0.20
s'arrêtera à la hauteur d'activation.

## 3. Programmer l'activation

Lire la hauteur actuelle (tableau de bord), et y ajouter **au moins 17 280**
(environ 24 h ; prendre 18 000 pour avoir de la marge).

Dans le **cockpit**, onglet **Gouvernance** (0.26 et plus ; onglet Actions
avant) : le tableau « Préparation des nœuds » doit montrer tous les nœuds
prêts ; « Hauteur conseillée » remplit la hauteur (actuelle + 18 000) et
affiche la date d'activation estimée. Signer avec deux clés admin, envoyer.

L'onglet Gouvernance du tableau de bord affiche alors la mise à jour
programmée et sa hauteur.

## 4. Pendant les 24 heures

Rien à faire. Mettre à jour les portefeuilles (PC : archive Windows 0.21.0 ;
téléphone : APK 0.21.0) pour qu'ils affichent la réserve après l'activation.

## 5. À la hauteur d'activation : vérifier

```bash
curl -s https://vinx-ledger.vinxlabs.fr/status | grep -oE '"(height|protocol_version)":[0-9]+'
curl -s https://vinx-ledger.vinxlabs.fr/accounts/TON-ADRESSE | grep -oE '"(objects|reserve|spendable)":[^,]*'
```

Attendu :

- `protocol_version` à **6**, et la hauteur continue d'avancer ;
- pour un compte qui a accepté un jeton : `objects` à 1 ou plus, `reserve` à
  `0,1 + 0,1 × objets` VINX (en unités : `100000000` par objet), `spendable`
  le reste ;
- dans le portefeuille, un envoi qui entamerait la réserve est refusé à
  l'aperçu, avec le montant à garder.

## En cas de problème

| Symptôme | Cause | Que faire |
|---|---|---|
| Un nœud s'arrête à la hauteur d'activation | il est resté en 0.20 | `install-validator.sh upgrade` avec la 0.21.0 : il repart et rattrape |
| La programmation est refusée (« mise à jour invalide ») | hauteur trop proche, ou version différente de 6 | reprogrammer avec au moins 17 280 blocs d'avance |
| Changer d'avis avant l'activation | — | une nouvelle programmation remplace la précédente (par exemple plus tard) |
