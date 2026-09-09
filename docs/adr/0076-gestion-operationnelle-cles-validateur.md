# ADR 0076 — Gestion opérationnelle des clés de validateur

- **Statut :** Implémenté 🔧 (socle) — rotation **Accepté 📐**, non codée
- **Date :** Septembre 2026
- **Portée :** Sécurité opérationnelle — matériel de clé sur un nœud en production
- **Décideur :** VinX Labs
- **Complète :** ADR 0012 (remote signer — futur), ADR 0052 (keystore portefeuille), ADR 0075 (enrôlement)
- **Crates :** `vinx-node` (`src/main.rs`)

---

## 1. Contexte

Un validateur détient trois secrets distincts, et jusqu'ici aucun n'avait de politique
écrite :

| Clé | Rôle | Conséquence d'une compromission |
|---|---|---|
| Ed25519 validateur | Signe les transactions du validateur, identifie l'adresse | Vol du bond, transactions frauduleuses |
| BLS12-381 | Co-signe les blocs (ADR 0046) | **Équivocation forgeable → slashing du bond** |
| Ed25519 admin | Gouvernance (si le nœud la porte) | Contrôle du set de validateurs et des upgrades |

Deux défauts concrets étaient présents :

- **Permissions par défaut** (finding VINX-15). `std::fs::write` crée les fichiers avec
  l'umask du processus, typiquement `0644` : la clé de signature d'un validateur était
  **lisible par tout utilisateur local** de la machine.
- **Clé BLS non persistée** (ADR 0075 §2.3) : régénérée à chaque démarrage.

ADR 0012 (remote signer, clé P2P distincte) reste la cible, mais est classée « futur ». Cet
ADR fixe le **minimum exigible pour lancer**, sans attendre 0012.

## 2. Décision

### 2.1 Le matériel de clé est écrit en `0600`

`write_secret_file` crée le fichier en `0600` **avant d'écrire le moindre octet** — la clé
n'est jamais brièvement lisible — et réaffirme le mode sur un fichier existant, qui
conserverait sinon ses anciennes permissions. S'applique aux clés validateur, admin, faucet
et BLS. Sur plateformes non-Unix, le comportement retombe sur `std::fs::write`, faute d'API
de permissions équivalente.

### 2.2 Chaque clé a un fichier, dans le `data_dir`

`validator.json`, `admin.json`, `validator_bls.json`, et le fichier faucet s'il est
configuré. Séparer les fichiers permet de ne sauvegarder ou déplacer que ce qui est
nécessaire, et rend explicite ce qu'un opérateur détient.

### 2.3 Les clés persistantes sont un invariant, pas une commodité

La clé BLS **doit** survivre aux redémarrages : la faire varier revient à invalider
l'enregistrement on-chain et à faire refuser tous les blocs du nœud (ADR 0075 §2.3).
Toute génération de clé au démarrage d'un composant signant est désormais considérée comme
un défaut.

### 2.4 Sauvegarde et restauration

L'opérateur doit sauvegarder `validator.json` et `validator_bls.json`. Perdre le premier,
c'est perdre le bond ; perdre le second, c'est devoir réenregistrer une clé BLS avant que
les blocs redeviennent acceptés.

**Contrainte de sûreté :** restaurer une sauvegarde de la clé BLS sur une **seconde**
machine qui produirait en parallèle est le moyen le plus simple de s'auto-slasher. Le verrou
de vote (ADR 0071) est **local à un nœud** : il ne protège pas contre deux nœuds partageant
la même clé. Un validateur ne doit jamais tourner en deux exemplaires actifs.

## 3. Ce qui reste ouvert

| Mesure | État | Note |
|---|---|---|
| Rotation de la clé BLS sans perdre le bond | 📐 Accepté | `RegisterBlsKey` écrase déjà l'entrée ; il manque la procédure et le délai de recouvrement pendant lequel les deux clés doivent être acceptées |
| Chiffrement au repos des clés du nœud | 💡 Proposé | Un validateur doit signer sans intervention humaine : une passphrase impose un déverrouillage au démarrage. À arbitrer contre le remote signer (ADR 0012) |
| Clé d'identité P2P distincte de la clé de consensus | 💡 Proposé | ADR 0012 |
| Remote signer / HSM | 💡 Proposé | ADR 0012 — cible réelle pour un validateur portant de la valeur |

## 4. Conséquences

- La documentation d'exploitation doit lister les fichiers à sauvegarder et l'interdiction
  formelle de faire tourner deux instances avec la même clé BLS.
- Le `data_dir` contient désormais des secrets : les images Docker et les volumes montés
  doivent être traités en conséquence, et le `data_dir` ne doit jamais être servi par
  l'API HTTP.

## 5. Critères de validation

- [x] Tout fichier de clé est créé en `0600`, y compris à la ré-écriture.
- [x] La clé BLS est chargée depuis le disque et non régénérée au démarrage.
- [ ] Procédure de rotation documentée et testée — **non codée**.
- [ ] Test d'intégration : redémarrer un validateur et vérifier que ses blocs restent
      acceptés par un pair — **à valider sur le banc** (ADR 0080).
