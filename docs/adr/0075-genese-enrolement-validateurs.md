# ADR 0075 — Cérémonie de genèse & enrôlement des validateurs

- **Statut :** Partiellement implémenté 🔧 — clé BLS au bonding **Accepté 📐**, non codé
- **Date :** Septembre 2026
- **Portée :** Amorçage — comment un réseau multi-validateurs démarre et s'ouvre
- **Décideur :** VinX Labs
- **Complète :** ADR 0033 (genèse fair-launch), ADR 0038 (admission au pool), ADR 0070 (authentification du proposeur)
- **Crates :** `vinx-state` (`src/genesis.rs`), `vinx-node` (`src/main.rs`)

---

## 1. Contexte

ADR 0070 rend l'enregistrement BLS on-chain **nécessaire** pour qu'un bloc soit accepté : on
ne peut pas vérifier la paternité d'un bloc contre une clé qu'on ne possède pas. Cette
exigence a révélé que rien n'existait pour l'honorer.

**L'interblocage d'amorçage.** `create_genesis_state` n'enregistrait aucune clé BLS, et
aucun chemin de code ne pouvait soumettre une transaction `RegisterBlsKey` — ni au
démarrage, ni via le CLI portefeuille. Sur un réseau multi-nœuds neuf, le registre restait
vide, les pairs refusaient donc tous les blocs, et la transaction qui aurait corrigé cela ne
pouvait voyager que **dans un bloc que les pairs acceptent**. Blocage circulaire.

**La clé BLS n'était pas persistée.** `NodeConfig::new` en générait une neuve à chaque
construction et `with_bls_key` n'était jamais appelé depuis `main.rs`. Un nœud signait donc
avec une clé BLS différente à chaque exécution : dès lors que les blocs sont authentifiés
contre une clé enregistrée, tous les blocs d'un nœud auraient été refusés **après son
premier redémarrage**, sans moyen de récupération.

Aucun de ces deux points n'apparaissait dans les rapports d'audit : ils n'ont émergé qu'en
corrigeant VINX-01.

## 2. Décision

### 2.1 La genèse enregistre la clé du validateur initial

`GenesisConfig` gagne `validator_bls: Option<GenesisBlsKey>` (clé G1 + PoP).
`create_genesis_state` **vérifie la PoP** puis écrit une entrée de pool pour le validateur
de genèse portant la clé — celui-ci étant dispensé de bond, il n'avait pas d'entrée.
Une PoP invalide **panique** plutôt que d'écrire dans l'état une clé qu'aucun bloc ne pourra
jamais vérifier.

`None` laisse le registre vide : utilisable seulement pour une chaîne mono-nœud, où le
producteur ajoute à sa propre chaîne sans passer par le contrôle P2P. Tout réseau
multi-nœuds doit la fournir.

### 2.2 La spec de genèse partagée transporte la clé

`GenesisSpec` (`VINX_GENESIS_SPEC`) gagne `initial_validator_bls_pub_key` et
`initial_validator_bls_pop`. Le validateur initial pouvant être un autre nœud, sa clé doit
venir de la spec partagée — chaque nœud doit dériver une genèse **identique**, sans quoi la
sync casse dès la hauteur 1. Une spec n'en fournissant qu'une moitié est rejetée ; n'en
fournissant aucune, elle avertit bruyamment.

### 2.3 La clé BLS est persistée

Stockée dans `<data_dir>/validator_bls.json` (clé secrète, clé publique, PoP), créée en
`0600` (ADR 0076), chargée au démarrage et injectée via `with_bls_key`. Elle doit être
stable à travers les redémarrages, exactement comme la clé Ed25519 du validateur.

### 2.4 Auto-enregistrement au démarrage

Un nœud bondé dont la clé enregistrée est absente ou périmée soumet automatiquement sa
propre `RegisterBlsKey`. C'est l'étape qui ferme la boucle : le validateur de genèse produit
des blocs — sa clé étant enregistrée — et ces blocs transportent les enregistrements des
autres validateurs, qui deviennent à leur tour authentifiables.

### 2.5 Unicité des clés BLS

Une clé G1 déjà détenue par un autre validateur est refusée à l'enregistrement (finding
VINX-11) : sans cela, deux index du bitmap résolvaient vers la même clé et une signature
réelle comptait deux fois.

## 3. Ce qui reste ouvert

### 3.1 Clé BLS exigée au bonding — 📐 Accepté, non codé

Un validateur peut aujourd'hui entrer au pool **sans** clé BLS puis l'enregistrer ensuite.
Tant que c'est le cas, « tout validateur actif est authentifiable » est une propriété de
**convergence**, pas un invariant. Exiger la clé (et sa PoP) dans la transaction de bond la
rendrait structurelle et supprimerait l'auto-enregistrement de §2.4.

### 3.2 PoP non liée à l'identité — 📐 Accepté, non codé

La PoP est signée sur `pk_bytes` seul (ADR 0046). Elle prouve la possession et **rien
d'autre** : elle n'est liée ni à l'adresse du validateur, ni au `chain_id`, donc rejouable
d'une chaîne à l'autre. La signer sur `bls_pub_key ‖ validator_address ‖ chain_id` change le
format de PoP — à faire en même temps que §3.1, les deux touchant la même transaction.

### 3.3 Cérémonie de genèse multi-validateurs

`GenesisConfig` n'admet **qu'un** validateur initial ; les autres rejoignent par bond. Pour
un lancement à plusieurs validateurs indépendants dès la hauteur 1, la genèse devra accepter
un ensemble de `(adresse, clé BLS, PoP)`, avec une procédure de collecte documentée et un
hash de genèse publié que chaque opérateur vérifie avant de démarrer.

## 4. Conséquences

- La documentation d'exploitation doit énoncer l'exigence : **un validateur doit avoir une
  clé BLS enregistrée avant que ses blocs soient acceptés.**
- `validator_bls.json` rejoint la liste des fichiers à sauvegarder : le perdre revient à
  perdre l'identité de signature du validateur jusqu'à un nouvel enregistrement.
- L'auto-enregistrement lit le nonce du compte au démarrage — comportement à réexaminer si
  le nœud redémarre en boucle ou si une transaction du même nonce est déjà en vol.

## 5. Critères de validation

- [x] La genèse enregistre la clé du validateur initial, rendant le registre non vide —
      `genesis_registers_the_validator_bls_key`.
- [x] Une PoP invalide est refusée à la genèse — `genesis_rejects_an_invalid_bls_pop`.
- [x] Une clé G1 déjà enregistrée par un autre validateur est refusée —
      `bls_key_cannot_be_registered_by_two_validators`.
- [x] La clé BLS survit à un redémarrage (fichier persisté).
- [ ] Un réseau à 3 validateurs démarre depuis une spec partagée et **tous** deviennent
      authentifiables sans action manuelle — **à valider sur le banc** (ADR 0080).
- [ ] Un bond sans clé BLS est refusé — **non codé** (§3.1).
- [ ] La PoP couvre adresse et `chain_id` — **non codé** (§3.2).
