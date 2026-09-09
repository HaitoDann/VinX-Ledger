# Audit de sécurité VinX-Ledger

**Commit audité :** `8774806` (branche `main`) — audit conduit sur `claude/vinx-ledger-security-audit-9f3zw9`
**Date :** 2026-09-09
**Périmètre :** `crates/vinx-crypto`, `crates/vinx-core`, `crates/vinx-state`, `crates/vinx-node`,
`crates/vinx-wallet`, interface web `rpc/ui.rs`. ~24 500 lignes de Rust.
**Méthode :** lecture intégrale des chemins consensus / état / P2P / RPC, puis écriture de
preuves de concept exécutables.

Les PoC vivent dans `crates/vinx-state/tests/audit_poc.rs` et
`crates/vinx-node/tests/audit_poc_node.rs`. Ils sont `#[ignore]` et **affirment la présence de
la faille** :

```
cargo test -p vinx-state --test audit_poc      -- --ignored
cargo test -p vinx-node  --test audit_poc_node -- --ignored
```

Les 7 PoC passent sur le commit audité.

> **Verdict.** La chaîne n'a, en pratique, **aucune authentification des blocs sur le chemin
> P2P**, et son `state_root` **ne couvre pas l'état de consensus**. Un attaquant sans clé de
> validateur peut fabriquer et faire finaliser des blocs. Un tiers peut vider n'importe quel
> compte via le champ `sponsor`. VinX-Ledger n'est pas en état d'être exposé à un réseau
> ouvert, ni à un mainnet portant de la valeur.

---

## Table des findings

| ID | Titre | Gravité | Confiance |
|----|-------|---------|-----------|
| VINX-01 | Aucune vérification de signature de bloc sur tout le chemin P2P | CRITICAL | CONFIRMED |
| VINX-02 | Quorum BLS falsifiable : repli `bls_cosigner_pks` quand le bitmap est vide | CRITICAL | CONFIRMED |
| VINX-03 | Signature du sponsor jamais vérifiée → vidage de n'importe quel compte | CRITICAL | CONFIRMED |
| VINX-04 | `state_root` ne couvre pas l'état de consensus (set, pool, admin, beacon) | CRITICAL | CONFIRMED |
| VINX-05 | Fork-choice et finalité pondérés par un compteur de co-signatures non lié au registre | HIGH | CONFIRMED |
| VINX-06 | Un seul validateur byzantin emprisonne (jail) tout le set honnête | HIGH | CONFIRMED |
| VINX-07 | `SyncResponse` sans borne de dérive d'horloge → saut de l'horloge protocole | HIGH | LIKELY |
| VINX-08 | `Chain::reorg_replace` ignore `height_base` → panique du nœud (crash distant) | HIGH | CONFIRMED |
| VINX-09 | `CompactBlock` : travail quadratique non borné → DoS CPU distant | HIGH | CONFIRMED |
| VINX-10 | Snapshot-sync : racine auto-cohérente, aucun lien avec l'en-tête ni la finalité | HIGH | CONFIRMED |
| VINX-11 | PoP BLS non liée à l'identité du validateur | MEDIUM | CONFIRMED |
| VINX-12 | `signing_bytes` ambigu à la frontière `payload` / `sponsor` | MEDIUM | CONFIRMED |
| VINX-13 | `payload` de transaction non borné → gonflement de bloc au prix plancher | MEDIUM | CONFIRMED |
| VINX-14 | `ScheduleUpgrade` par gouvernance contourne le délai de préavis ADR 0006 | MEDIUM | CONFIRMED |
| VINX-15 | Clés privées du nœud écrites en clair avec les permissions par défaut | MEDIUM | CONFIRMED |
| VINX-16 | Candidats de fork-choice non bornés en mémoire | MEDIUM | LIKELY |
| VINX-17 | ECVRF : `validate_key` RFC 9381 absente ; beacon d'époque sans entropie | MEDIUM | CONFIRMED |
| VINX-18 | Sélection de comité VRF non câblée + classement indépendant du beacon | MEDIUM | CONFIRMED |
| VINX-19 | UI web : clé privée manipulée sous un script CDN sans SRI ni CSP | MEDIUM | CONFIRMED |
| VINX-20 | Autorité admin « fail-open » quand aucun admin n'est configuré | LOW | CONFIRMED |
| VINX-21 | Réputation des pairs monotone décroissante → bannissement d'honnêtes | LOW | CONFIRMED |
| VINX-22 | Fuites mémoire non bornées (`faucet_cooldowns`, `seen`, index) | LOW | LIKELY |
| VINX-23 | `try_evict_for` : O(n) sur mempool plein à chaque insertion | LOW | CONFIRMED |
| VINX-24 | Ed25519 : `verify` non strict (clés d'ordre faible acceptées) | INFO | CONFIRMED |

---

## VINX-01

**Titre :** Aucune vérification de signature de bloc sur tout le chemin P2P
**Gravité :** CRITICAL
**Confiance :** CONFIRMED
**Fichier :** `crates/vinx-node/src/p2p/mod.rs`
**Fonction :** `dispatch_message` — bras `P2pMessage::NewBlock`, `CompactBlock`, `TxResponse`, `SyncResponse`
**Lignes :** 561–870 (NewBlock : 584, 666, 731, 737, 784) ; 1112–1215 (CompactBlock) ; 1251–1305 (TxResponse) ; 1341–1420 (SyncResponse)

### Description
Le bras `NewBlock` enchaîne exactement cinq contrôles : (1) chaînage `prev_hash` + bornes de
timestamp, (2) appartenance du proposeur au `ValidatorSet`, (3) signatures **de transactions**,
(4) transition d'état + `state_root` + invariant de masse, (5) commit. **Aucun d'eux ne vérifie
`bls_aggregate`.** Ni `consensus::validate_block` ni `consensus::validate_block_with_registry`
n'est appelé depuis `p2p/mod.rs` — un `grep` sur le crate le confirme : les seuls appelants sont
`sync.rs:129` et `sync.rs:362`.

Or l'en-tête de bloc ne porte **aucune signature Ed25519 du proposeur** : `BlockHeader` (block.rs
:13-28) n'a pas de champ signature, et le type `BlockSignature` (block.rs:47) n'est utilisé nulle
part dans le code de production. La seule preuve d'autorité d'un bloc est donc l'agrégat BLS…
que le chemin P2P ne regarde pas.

`header.validator` est une adresse publique. N'importe qui peut donc composer un bloc en s'y
déclarant proposeur.

### Preuve dans le code
```rust
// p2p/mod.rs:666  — seul contrôle d'autorité
if !vs.contains(&block.header.validator) {
    warn!(height, "P2P block from non-validator proposer");
    return;
}
// p2p/mod.rs:731  — puis on passe directement aux transactions
if !verify_block_tx_signatures_parallel(&block.transactions) { ... }
// p2p/mod.rs:784  — puis commit. bls_aggregate n'a jamais été lu.
c.push(block.clone());
```
Le chemin `CompactBlock` est pire : il reconstruit un `Block` avec
`bls_aggregate: None, bls_cosigner_pks: vec![], bls_bitmap: vec![]` (mod.rs:1178-1184 et
1272-1278) puis le ré-injecte dans `NewBlock`. Un bloc **sans aucune signature** est appliqué.

### Scénario d'exploitation
1. L'attaquant, simple pair gossip sans aucune clé, lit `GET /validators` pour obtenir l'adresse
   d'un validateur honnête `V` et `GET /chain/height` pour le tip.
2. Il construit un bloc à `tip+1` : `prev_hash` = tip, `timestamp` = maintenant,
   `validator = V`, transactions de son choix (signées valablement — les siennes suffisent).
3. Il rejoue localement la transition pour calculer le bon `state_root`.
4. Il publie `CompactBlock` (ou `NewBlock`) sur `vinx/compact/1`.
5. Tous les nœuds l'appliquent et l'ajoutent à leur chaîne.

### Impact
Contrôle total de l'ordonnancement et de la censure des transactions par un acteur
non-validateur ; attribution de la récompense d'émission au validateur nommé ; fabrication
d'historique. Combiné à VINX-02/VINX-05, le bloc forgé est aussi **finalisé**.

### Correction recommandée
Appeler `validate_block_with_registry` (corrigé selon VINX-02) sur **chaque** bloc entrant,
avant tout travail d'état, dans les quatre bras (`NewBlock`, `SyncResponse`, et sur le bloc
reconstruit des chemins compacts). Ajouter en complément une signature Ed25519 du proposeur sur
`header.hash()`, vérifiée contre `validator_set`, afin que la paternité d'un bloc soit prouvée
même avant que le quorum de co-signatures ne soit atteint.

### Test permettant de confirmer/réfuter
Test d'intégration : monter deux nœuds, faire publier par un troisième processus **non
validateur** un `NewBlock` valide côté état mais avec `bls_aggregate: None`, et asserter que
`chain.tip_height()` n'a pas bougé. Aujourd'hui il bouge.

---

## VINX-02

**Titre :** Quorum BLS falsifiable — repli sur `bls_cosigner_pks` quand `bls_bitmap` est vide
**Gravité :** CRITICAL
**Confiance :** CONFIRMED
**Fichier :** `crates/vinx-core/src/block.rs`, `crates/vinx-node/src/consensus.rs`
**Fonction :** `Block::bls_signer_count_from_bitmap`, `Block::bls_signer_count`, `Block::is_finalized`, `consensus::validate_block_with_registry`
**Lignes :** block.rs 140-173 (repli ligne 144-146), 178-204, 207-214 ; consensus.rs 57-93

### Description
`validate_block_with_registry` est présenté comme le contrôle « lié au registre » : chaque bit du
bitmap doit correspondre à une clé BLS enregistrée on-chain. Mais son cœur,
`bls_signer_count_from_bitmap`, commence par :

```rust
if self.bls_bitmap.is_empty() {
    return self.bls_signer_count();   // block.rs:144-146
}
```

et `bls_signer_count` vérifie l'agrégat contre `self.bls_cosigner_pks`, c'est-à-dire **des clés
publiques fournies par l'attaquant dans le bloc lui-même**, sans passage par le registre et sans
PoP. Le compte de signataires retourné est simplement `pks.len()`.

Un attaquant met `bls_bitmap = vec![]`, génère `quorum` paires de clés BLS, signe le hash
d'en-tête avec chacune, agrège, et place les clés publiques dans `bls_cosigner_pks`. Le quorum est
atteint. Le registre n'est jamais consulté.

Le commentaire justifie le repli par la compatibilité avec les « blocs pré-ADR-0029 ». C'est un
mode de dégradation choisi par l'attaquant : il suffit d'omettre un champ pour désactiver la
sécurité.

### Preuve dans le code
PoC `poc_forged_quorum_via_empty_bitmap` (`crates/vinx-node/tests/audit_poc_node.rs`) : set de
7 validateurs (quorum 5), registre `vec![None; 7]` (**aucun** validateur n'a de clé BLS
enregistrée), bloc à bitmap vide signé par 5 clés générées par l'attaquant →
`validate_block_with_registry(...).expect(...)` **réussit**, et `block.is_finalized(&vs)` est vrai.

### Scénario d'exploitation
Le nœud victime démarre avec `sync_peer_rpc` ou reçoit des `SyncResponse` : un pair malveillant
sert une histoire entièrement fabriquée dont chaque bloc porte un faux quorum. Les contrôles
restants (`prev_hash`, timestamps, signatures de tx, `state_root`) sont tous satisfaisables par
l'attaquant puisqu'il construit lui-même la branche.

### Impact
Finalité forgée. Un attaquant sans participation au consensus produit une chaîne que les nœuds
considèrent comme irréversible (`finalized_height` avance, cf. `chain.rs:172` qui utilise aussi
`bls_signer_count()`). Perte totale de sûreté.

### Correction recommandée
Supprimer purement et simplement le repli et le champ `bls_cosigner_pks` du format de bloc :
rejeter tout bloc non-genesis dont `bls_bitmap` est vide. Faire de `bls_signer_count_from_bitmap`
l'unique chemin, et supprimer `bls_signer_count` / `is_finalized` (ou les réécrire pour exiger le
registre). Si un support des anciens blocs est nécessaire, le borner par une hauteur d'activation
codée en dur, pas par un champ du bloc.

### Test permettant de confirmer/réfuter
Le PoC ci-dessus. Après correctif, il doit échouer avec `BlsError`.

---

## VINX-03

**Titre :** Signature du sponsor jamais vérifiée sur le chemin mempool → vidage de n'importe quel compte
**Gravité :** CRITICAL
**Confiance :** CONFIRMED
**Fichier :** `crates/vinx-state/src/world_state.rs`, `crates/vinx-node/src/mempool.rs`, `crates/vinx-node/src/rpc/handlers.rs`
**Fonction :** `WorldState::admission_check`, `Mempool::verify_sig_static`, `handlers::submit_tx`, `handlers::submit_tx_batch`, `WorldState::apply_transaction_trusted` → `apply_transfer`
**Lignes :** world_state.rs 1180-1247 (admission), 1291-1295 (trusted), 1400-1407 + 1455-1463 (débit du sponsor) ; mempool.rs 368-381 ; handlers.rs 114-135, 795-820

### Description
Une transaction sponsorisée porte trois champs supplémentaires (`sponsor`, `sponsor_pub_key`,
`sponsor_signature`) et, à l'application, **le fee est débité du sponsor** au lieu de
l'émetteur. La seule fonction qui vérifie la signature du sponsor est
`WorldState::verify_tx_signature_pure` (world_state.rs:1340-1355).

Or aucun des trois points d'entrée du mempool ne l'appelle :

* `handlers::submit_tx` (handlers.rs:118-133) vérifie `pub_key`/`signature` de l'**émetteur
  seulement** ;
* `handlers::submit_tx_batch` (handlers.rs:800-818) fait exactement la même chose ;
* `Mempool::verify_sig_static` (mempool.rs:368-381), utilisé par `flush_staged` pour les
  transactions arrivées par gossip, également.

`admission_check` ne fait que constater que le compte sponsor existe et couvre le fee
(world_state.rs:1237-1246). La transaction entre alors dans `Mempool::queues`, dont l'invariant
documenté est « toutes les entrées ont une signature valide ». Le producteur la sort via
`drain()` et l'applique avec `apply_transaction_trusted`, qui **saute** délibérément la
vérification de signature.

Aucune borne supérieure n'existe sur `fee` : `apply_transfer` n'exige que `fee >= base_fee`
(world_state.rs:1390-1397). Le fee peut donc valoir le solde entier de la victime, et il est
crédité au producteur du bloc via `block_fees` → `settle_block`.

### Preuve dans le code
PoC `poc_sponsor_signature_never_checked` (`crates/vinx-state/tests/audit_poc.rs`) : le compte
victime, crédité de 5 000 VINX, tombe à zéro après une transaction dont
`sponsor_signature: None`. `admission_check` la valide, `apply_transaction_trusted` l'applique,
et `verify_tx_signature_pure` la rejetterait — mais n'est jamais appelée.

### Scénario d'exploitation
1. L'attaquant crée un compte financé (n'importe quel montant ≥ ED).
2. Il construit un `Transfer` de 0 VINX vers lui-même, `sponsor = <adresse victime>`,
   `fee = <solde exact de la victime>`, `sponsor_pub_key = None`, `sponsor_signature = None`.
3. Il signe uniquement de sa propre clé et poste sur `POST /tx/submit` (ou gossippe
   `NewTransaction`).
4. Le producteur l'inclut : la victime est vidée, le producteur encaisse le fee.

### Impact
Vol direct de fonds : n'importe quel solde, sans consentement ni interaction de la victime,
la seule information requise étant son adresse publique.

Effet secondaire tout aussi grave : le bloc produit contient une transaction que les autres
nœuds **rejettent** (`verify_block_tx_signatures_parallel` appelle bien
`verify_tx_signature_pure`). Le producteur se retrouve donc sur une branche que personne
n'accepte. En envoyant cette transaction au leader courant à chaque hauteur, un attaquant
**arrête la production de blocs du réseau**.

### Correction recommandée
Remplacer toutes les vérifications ad-hoc de signature (handlers.rs:118-133 et 800-818,
mempool.rs:368-381) par un appel unique à `WorldState::verify_tx_signature_pure`. C'est la
fonction canonique ; la dupliquer partiellement est précisément ce qui a créé la faille.
Ajouter en défense en profondeur un plafond sur `fee` (par ex. `fee <= base_fee * K`) et
faire de `apply_transaction_trusted` une fonction `unsafe`-par-convention documentée avec un
`debug_assert!(verify_tx_signature_pure(tx).is_ok())`.

### Test permettant de confirmer/réfuter
Le PoC ci-dessus. Après correctif : `admission_check` ou l'insertion mempool doit refuser, et
un test symétrique doit couvrir `submit_tx`, `submit_tx_batch` et `flush_staged`.

---

## VINX-04

**Titre :** `state_root` ne couvre que les comptes — l'état de consensus n'est pas engagé
**Gravité :** CRITICAL
**Confiance :** CONFIRMED
**Fichier :** `crates/vinx-state/src/world_state.rs`
**Fonction :** `hash_account`, `WorldState::compute_state_root`
**Lignes :** 2426-2434 (`hash_account`), 1717-1720 (`compute_state_root`), 29-200 (champs de `WorldState`)

### Description
La feuille Merkle d'un compte (world_state.rs:2426-2434) est
`sha256(address(20) ‖ balance(16 BE) ‖ nonce(8 BE) ‖ staked(16 BE))`. La racine ne couvre donc
**rien d'autre**. Sont hors engagement :

`validator_set`, `validator_pool` (bonds, `bls_pub_key`, `bls_pop`, `vrf_pub_key`, statuts),
`banned_validator_keys`, `admin_address`, `admin_policy`, `pending_governance`,
`pending_upgrade`, `current_version`, `pending_unbonds`, `exit_queue`, `reliability`,
`epoch_beacon`, `modules`, `base_fee`, `fee_floor`, `emitted_atoms`, `circulating_supply`,
`destroyed_atoms`, `epoch_dist_emission_pot`, `active_set_size`, `min_validator_bond_atoms`,
`chain_id`.

Le `state_root` est pourtant le **seul** contrôle d'intégrité d'état de tous les chemins de
validation de bloc (p2p/mod.rs:764, sync.rs:180, reorg.rs:62). Deux nœuds peuvent donc diverger
sur l'ensemble des validateurs, la clé admin, le registre BLS ou le beacon d'époque tout en
publiant le même `state_root` — la divergence est **silencieuse et indétectable**.

### Preuve dans le code
PoC `poc_state_root_does_not_commit_to_consensus_state` : deux `WorldState` dont les comptes sont
identiques mais dont le `validator_set`, `admin_address`, `epoch_beacon`, `validator_pool`,
`min_validator_bond_atoms` et `active_set_size` diffèrent produisent des racines **égales**.

### Scénario d'exploitation
Ce défaut n'est pas exploitable seul : il transforme tout bug de déterminisme dans la partie
non couverte en fork silencieux, et il ôte toute valeur au contrôle du snapshot-sync
(cf. VINX-10) — un snapshot peut installer un `validator_set` et une `admin_policy` arbitraires
tout en présentant une racine « vérifiée ». Il empêche également toute vérification par un
client léger : la preuve Merkle exposée par `GET /account/:addr/proof` ne dit rien du set de
validateurs qui l'a signée.

### Impact
Perte de la propriété fondamentale « la racine engage l'état ». Fork silencieux, absence de
détection d'un nœud corrompu, snapshot-sync non vérifiable.

### Correction recommandée
Faire de `state_root` une racine sur **deux** sous-arbres :
`state_root = sha256(accounts_root ‖ consensus_root)`, où `consensus_root` est une racine Merkle
(ou un simple hash canonique) sur l'encodage déterministe de tous les champs de consensus listés
ci-dessus. Verrouiller ensuite l'ensemble par un vecteur de test golden, comme
`test_signing_bytes_golden_vector` le fait pour les transactions. Changement cassant le
consensus : à faire avant tout lancement.

### Test permettant de confirmer/réfuter
Le PoC ci-dessus, à inverser (`assert_ne!`) après correctif, plus un test par champ de consensus
mutant l'état et vérifiant que la racine change.

---

## VINX-05

**Titre :** Fork-choice et finalité pondérés par un compteur de co-signatures non lié au registre
**Gravité :** HIGH
**Confiance :** CONFIRMED
**Fichier :** `crates/vinx-node/src/consensus.rs`, `crates/vinx-node/src/chain.rs`
**Fonction :** `consensus::signer_weight`, `consensus::more_canonical`, `Chain::advance_finality`
**Lignes :** consensus.rs 180-182, 187-201 ; chain.rs 158-178 (ligne 172)

### Description
La règle 3 de l'ADR 0031 (« poids de co-signatures ») utilise
`signer_weight = block.bls_signer_count().unwrap_or(0)` — le chemin `bls_cosigner_pks`, non lié
au registre (VINX-02). De même, `Chain::advance_finality` décide de la finalité avec
`b.bls_signer_count().map(|c| c >= threshold)` (chain.rs:172).

Le poids de fork-choice est donc **un nombre choisi par l'auteur du bloc**.

### Preuve dans le code
```rust
// consensus.rs:180-182
fn signer_weight(block: &Block, _vs: &ValidatorSet) -> usize {
    block.bls_signer_count().unwrap_or(0)
}
// chain.rs:172
Some((_, b)) => b.bls_signer_count().map(|c| c >= threshold).unwrap_or(false),
```
`bls_signer_count` (block.rs:191-203) construit les clés depuis `self.bls_cosigner_pks` et
retourne `pks.len()` — aucune borne sur le nombre de clés, aucune vérification d'appartenance
au set, aucune vérification de PoP.

### Scénario d'exploitation
Un attaquant publie à une hauteur contestée un bloc concurrent portant 1 000 clés BLS qu'il a
générées et un agrégat valide de 1 000 signatures. `signer_weight` = 1 000 contre 1 pour le bloc
honnête → `more_canonical` élit systématiquement le bloc de l'attaquant, et `advance_finality`
le finalise puisque 1 000 ≥ quorum. `consider_competing_block` (p2p/mod.rs:495-518) ne contrôle
que l'appartenance du proposeur et les signatures de transactions avant d'entrer dans le
fork-choice.

### Impact
Réorganisation à volonté sous la profondeur de finalité, puis finalisation forgée. Combiné à
VINX-01, un non-validateur gagne durablement chaque course de fork-choice.

### Correction recommandée
Remplacer `signer_weight` et le prédicat de `advance_finality` par
`bls_signer_count_from_bitmap(&state.indexed_bls_keys(vs))` (donc en passant l'état au
fork-choice), après suppression du repli VINX-02. Borner le poids par `validator_set.len()`.

### Test permettant de confirmer/réfuter
Construire deux blocs concurrents, l'un signé par 2 validateurs enregistrés, l'autre par
50 clés inconnues, et asserter que `canonical_head` élit le premier.

---

## VINX-06

**Titre :** Un seul validateur byzantin emprisonne (jail) l'intégralité du set honnête
**Gravité :** HIGH
**Confiance :** CONFIRMED
**Fichier :** `crates/vinx-core/src/reliability.rs`, `crates/vinx-node/src/p2p/mod.rs`
**Fonction :** `reliability::on_block_applied`, `reliability::active_validators`, bras `NewBlock`
**Lignes :** reliability.rs 88-107, 57-68 ; p2p/mod.rs 666-671 ; world_state.rs 577-586 (appel depuis `settle_block`)

### Description
`on_block_applied` remet à zéro le compteur du proposeur effectif et **incrémente un
« manquement » au leader prévu** dès que le proposeur diffère. Trois manquements consécutifs
(`MAX_MISSED_PROPOSALS = 3`) emprisonnent le leader, qui sort de la rotation jusqu'à une
transaction `Unjail` suivie de 100 blocs de cooldown.

Or le chemin d'acceptation P2P n'exige **pas** que le proposeur soit le leader prévu : il
accepte n'importe quel membre du set (p2p/mod.rs:666-671), sans contrôle de délai de créneau
(« slot timeout »). Rien ne distingue donc un backup légitime d'un validateur qui court-circuite
systématiquement le leader.

### Preuve dans le code
PoC `poc_single_byzantine_validator_jails_the_whole_honest_set` : sur un set de 5 (4 honnêtes +
1 attaquant), 60 blocs tous proposés par l'attaquant suffisent pour que
`reliability::active_validators` ne retourne plus que `[attacker]` et que les 4 honnêtes soient
`is_jailed()`.

### Scénario d'exploitation
L'attaquant bonde un seul validateur (100 000 VINX), puis publie son bloc à chaque hauteur avant
le leader prévu — ce que VINX-01/VINX-05 rendent trivial (son bloc gagne le fork-choice grâce à
un poids de co-signatures forgé). Après trois tours, chaque honnête est jailé. La rotation
`active_leader_at` ne contient plus que lui.

### Impact
Prise de contrôle complète de la production de blocs par un validateur unique : 100 % de
l'émission proposeur, censure totale, ordonnancement arbitraire. Les honnêtes doivent émettre
une `Unjail` et attendre 100 blocs — pendant lesquels l'attaquant les re-jaile.

Note d'atténuation : le quorum de finalité reste calculé sur le set **complet**
(`vs.quorum()`, cf. commentaire de sûreté producer.rs:69-77), donc le jailing seul ne casse pas
la sûreté ; il casse la vivacité et l'équité.

### Correction recommandée
N'attribuer un manquement que lorsqu'un **délai de créneau** vérifiable est écoulé :
`block.header.timestamp >= slot_start(height) + SLOT_TIMEOUT_SECS`, quantité déterministe
dérivable de l'en-tête. Refuser un bloc d'un non-leader dont le timestamp est antérieur à ce
seuil. Ajouter un plafond de jailings par époque, et exiger que l'auteur du bloc pré-emptif ne
soit pas systématiquement le même (par ex. rotation des backups).

### Test permettant de confirmer/réfuter
Le PoC ci-dessus. Après correctif : mêmes 60 blocs proposés « trop tôt » par l'attaquant → aucun
honnête jailé, et les blocs eux-mêmes rejetés.

---

## VINX-07

**Titre :** `SyncResponse` sans borne de dérive d'horloge — l'horloge protocole peut être projetée dans le futur
**Gravité :** HIGH
**Confiance :** LIKELY
**Fichier :** `crates/vinx-node/src/p2p/mod.rs`
**Fonction :** `dispatch_message` — bras `P2pMessage::SyncResponse`
**Lignes :** 1366-1371 (contrôle de monotonie), absence du contrôle `MAX_CLOCK_DRIFT_SECS`

### Description
Les trois autres chemins d'ingestion de blocs bornent le timestamp par
`now + MAX_CLOCK_DRIFT_SECS` : `NewBlock` (p2p/mod.rs:657-664), `sync_from_peer`
(sync.rs:110-117), `parallel_sync_from_peer` (sync.rs:345-350). Le bras `SyncResponse` ne
contrôle **que** la monotonie :

```rust
// p2p/mod.rs:1366-1371
if block.header.timestamp <= chain.read().await.tip_timestamp() {
    warn!(height, "SyncResponse block timestamp not monotonic");
    break;
}
```

`SyncResponse` est un message gossip librement publiable, portant jusqu'à
`MAX_SYNC_RESPONSE_BLOCKS = 512` blocs.

L'horloge protocole est le Median Time Past sur `MEDIAN_TIME_BLOCKS = 11` blocs
(chain.rs:337-347) : un seul bloc ne peut pas la faire sauter, mais **six blocs consécutifs**
suffisent à déplacer la médiane. `emit_work_reward` (world_state.rs:990-1023) émet
`cumulative_emission_atoms(now - emission_epoch_ts) - emitted_atoms`, et `mature_unbonds`
(world_state.rs:968-987) libère tous les bonds dont `unlock_ts <= block_ts`.

### Preuve dans le code
Comparaison directe des quatre bras : le contrôle de dérive figure en p2p/mod.rs:657, sync.rs:110
et sync.rs:345, et est absent en p2p/mod.rs:1366.

### Scénario d'exploitation
L'attaquant (combiné à VINX-01 pour l'acceptation des blocs) publie un `SyncResponse` de 6 blocs
contigus au-dessus du tip, de timestamps croissants situés dans 40 ans. Le MTP saute d'autant.
`cumulative_emission_atoms` retourne alors `MAX_SUPPLY_ATOMS` : la totalité de l'émission
restante est frappée en un bloc, 20 % au proposeur nommé, 80 % dans le `epoch_dist_emission_pot`.
Tous les `pending_unbonds` mûrissent immédiatement, annulant le délai de 3 jours pendant lequel
les bonds restent slashables.

L'invariant de masse reste satisfait (`mint_emission` le maintient), et le `state_root` est
calculé par l'attaquant : aucun garde-fou ne se déclenche.

### Impact
Inflation instantanée de la totalité de l'offre restante ; contournement du délai de
dé-bonding (un validateur peut équivoquer puis récupérer son bond avant que la preuve n'arrive).

### Correction recommandée
Factoriser les contrôles de bornes de timestamp dans une seule fonction
`validate_header_timing(header, chain, now) -> Result<()>` et l'appeler depuis les quatre
chemins. Ajouter, en défense en profondeur, un plafond dur sur la progression du MTP entre deux
blocs (par ex. `mtp_next <= mtp_prev + MAX_BLOCK_TIME_JUMP_SECS`).

### Test permettant de confirmer/réfuter
Test d'intégration : injecter un `SyncResponse` de 6 blocs à `timestamp = now + 10 ans` sur un
nœud, puis asserter que `state.emitted_atoms` n'a pas bougé. Marqué LIKELY car non exécuté ici :
le scénario complet requiert le harnais réseau.

---

## VINX-08

**Titre :** `Chain::reorg_replace` indexe par hauteur absolue en ignorant `height_base` → panique du nœud
**Gravité :** HIGH
**Confiance :** CONFIRMED
**Fichier :** `crates/vinx-node/src/chain.rs`
**Fonction :** `Chain::reorg_replace`
**Lignes :** 374-401 (ligne 386 : `let h = height as usize; ... self.blocks.drain(h..)`)

### Description
Toutes les autres méthodes convertissent correctement hauteur → index
(`height.checked_sub(self.height_base)`, cf. `block_row` chain.rs:136-139 et `get_block`
chain.rs:396-399). `reorg_replace` ne le fait pas :

```rust
let h = height as usize;                                   // chain.rs:386
let removed: Vec<Hash32> = self.blocks.drain(h..)...;       // chain.rs:389
```

Sur une chaîne amorcée par snapshot-sync, `height_base` vaut la hauteur du snapshot
(`Chain::new_from_snapshot`, chain.rs:107-121) et `blocks.len()` est petit. `drain(h..)` avec
`h > len` **panique** (`start index out of range`). La valeur est de plus persistée et restaurée
au redémarrage (`storage.rs:585`), donc la condition survit aux reboots.

### Preuve dans le code
PoC `poc_reorg_replace_panics_on_snapshot_synced_chain` : chaîne à `height_base = 1000` et
2 blocs, `reorg_replace(1001, ...)` panique avec `out of range`.

`median_time_past_ending_at` (chain.rs:355-365) présente le même défaut
(`let end = (height as usize).min(self.blocks.len())`) : il ne panique pas mais calcule une
fenêtre MTP fausse sur une chaîne snapshot, ce qui produit un `state_root` divergent au rejeu de
réorg.

### Scénario d'exploitation
Un nœud amorcé par snapshot reçoit un bloc concurrent valide à une hauteur non finalisée
(situation normale sur slot-skip, ou provoquée par un attaquant via VINX-01).
`consider_competing_block` → `reorg::consider_candidate` → `chain.reorg_replace` → panique. Le
processus meurt.

### Impact
Déni de service distant sur tous les nœuds amorcés par snapshot — c'est-à-dire la totalité des
nœuds rejoignant une chaîne mature. Un attaquant tue le réseau en publiant un bloc concurrent.

### Correction recommandée
```rust
let Some(idx) = height.checked_sub(self.height_base) else { return vec![] };
let idx = idx as usize;
if idx >= self.blocks.len() { return vec![]; }
let removed = self.blocks.drain(idx..)...;
```
Corriger identiquement `median_time_past_ending_at`. Ajouter un test paramétré sur
`height_base ∈ {0, 1000}` pour toutes les méthodes de `Chain` qui indexent `blocks`.

### Test permettant de confirmer/réfuter
Le PoC ci-dessus, à convertir en test nominal (réorg réussie) après correctif.

---

## VINX-09

**Titre :** `CompactBlock` — résolution des transactions en O(hashes × mempool) sans borne
**Gravité :** HIGH
**Confiance :** CONFIRMED
**Fichier :** `crates/vinx-node/src/p2p/mod.rs`, `crates/vinx-node/src/mempool.rs`
**Fonction :** bras `P2pMessage::CompactBlock` ; `Mempool::pending_txs`, `Mempool::get_by_hashes`
**Lignes :** p2p/mod.rs 1133-1148 (ligne 1140) ; mempool.rs 338-340, 344-356

### Description
```rust
// p2p/mod.rs:1136-1145
for &h in &tx_hashes {
    let found = mp.pending_txs().into_iter().find(|tx| tx.hash() == h).cloned();
    ...
}
```
`pending_txs()` **alloue un `Vec` de tout le mempool** (jusqu'à `DEFAULT_MAX_SIZE = 100 000`
entrées) à **chaque itération**, et `tx.hash()` recalcule un SHA-256 sur `signing_bytes()` pour
chaque transaction comparée.

`tx_hashes` n'est borné par rien : ni par `MAX_TX_REQUEST_HASHES` (qui n'est appliqué qu'aux
`TxRequest`/`TxResponse`, p2p/mod.rs:1201, 1219, 1258), ni par `header.tx_count`. La seule limite
est `MAX_DECODED_BYTES = 16 MiB`, soit **524 288 hashes**. Aucun contrôle d'appartenance du
proposeur au set n'est effectué avant ce travail.

`Mempool::get_by_hashes` (bras `TxRequest`) présente une variante plus faible : `O(mempool)`
itérations avec un `hashes.contains(&h)` linéaire à l'intérieur (mempool.rs:344-356).

### Preuve dans le code
Voir les extraits ci-dessus. Le garde anti-flood (`guard::PeerGuard`, 50 msg/s, burst 200)
compte les **messages**, pas le travail : un seul message autorisé déclenche l'explosion.

### Scénario d'exploitation
Un pair publie sur `vinx/compact/1` un `CompactBlock` dont `header.height = tip+1` et
`tx_hashes` contient 524 288 hashes aléatoires. Avec un mempool de 100 000 entrées, cela
représente ~5×10¹⁰ calculs de SHA-256 et 524 288 allocations d'un `Vec` de 100 000 pointeurs, le
tout sous le verrou `mempool.read()`. Le nœud est bloqué. Le message compressé ne pèse que
quelques centaines de kilo-octets en zstd.

### Impact
Déni de service CPU/mémoire distant, sur un message unique, contre tous les nœuds abonnés au
topic. Le verrou mempool étant tenu, la production de blocs et le RPC sont gelés en même temps.

### Correction recommandée
1. Rejeter tout `CompactBlock` dont `tx_hashes.len() > MAX_BLOCK_TXS` et dont
   `tx_hashes.len() as u32 != header.tx_count`.
2. Vérifier `vs.contains(&header.validator)` (et VINX-01) **avant** toute résolution.
3. Ajouter au mempool un index `AHashMap<Hash32, (Address, u64)>` et résoudre en O(1) par hash
   au lieu de scanner ; supprimer `pending_txs()` du chemin chaud.
4. Passer `get_by_hashes` sur un `HashSet` d'entrée.
5. Contrôler `header.receipts_root == sha256(tx_hashes concaténés)` — le format le permet déjà
   (`producer::compute_receipts_root`) et ce champ n'est aujourd'hui **jamais revérifié**.

### Test permettant de confirmer/réfuter
Benchmark : mempool de 50 000 transactions, `CompactBlock` de 100 000 hashes inconnus, mesurer
le temps de `dispatch_message`. Attendu après correctif : < 10 ms ; aujourd'hui : plusieurs
minutes.

---

## VINX-10

**Titre :** Snapshot-sync — la racine vérifiée est auto-cohérente et non liée à l'en-tête ni à la finalité
**Gravité :** HIGH
**Confiance :** CONFIRMED
**Fichier :** `crates/vinx-node/src/sync.rs`, `crates/vinx-node/src/chain.rs`
**Fonction :** `snapshot_sync_from_peer`, `Chain::new_from_snapshot`
**Lignes :** sync.rs 532-556 (contrôle de racine), 549-553 (installation) ; chain.rs 107-121

### Description
```rust
// sync.rs:533-547
let computed_root = new_state.compute_state_root();
let expected_root = hex::decode(&snap.state_root)...;
if computed_root.as_ref() != expected_root.as_slice() { return false; }
```
`snap.state_root` **provient du même pair** que `snap.state_hex`. Le contrôle démontre seulement
que le pair sait calculer une racine sur l'état qu'il vient d'envoyer. Il n'est comparé ni à
`snap.block.header.state_root` (pourtant présent dans la réponse et gratuit à vérifier), ni à
une racine obtenue d'une source indépendante.

Ensuite `Chain::new_from_snapshot` fixe `finalized_height = height` : le bloc du snapshot est
déclaré **définitivement final** sans qu'aucun quorum n'ait été vérifié.

Combiné à VINX-04, la racine ne couvre de toute façon ni le `validator_set`, ni `admin_address`,
ni `validator_pool` : même un contrôle correct contre l'en-tête ne protégerait pas ces champs.

### Preuve dans le code
Voir extrait. `snap.block` est utilisé uniquement pour construire la chaîne (sync.rs:551), jamais
pour valider l'état. Aucun appel à `validate_block_with_registry` sur ce bloc.

### Scénario d'exploitation
`sync_peer_rpc` est une URL de configuration, potentiellement en HTTP simple (aucune contrainte
TLS, aucun pinning : `reqwest::Client::builder()` sans configuration, sync.rs:442-445). Un
attaquant en position réseau, ou un opérateur de bootstrap malveillant, sert un `WorldState`
fabriqué : `validator_set` = ses propres adresses, `admin_address` = la sienne,
`validator_pool` peuplé de ses clés BLS, soldes arbitraires. Le nœud l'adopte et le marque
finalisé.

### Impact
Compromission totale du nœud amorcé : il suit une chaîne contrôlée par l'attaquant, avec un set
de validateurs et une autorité de gouvernance choisis par lui.

### Correction recommandée
1. Exiger `computed_root == snap.block.header.state_root` (et non `snap.state_root`, à supprimer
   du protocole).
2. Valider `snap.block` avec `validate_block_with_registry` contre le `validator_set` du snapshot
   **et** contre un jeu de checkpoints de confiance codés en dur (hash d'en-tête + hauteur) livrés
   avec le binaire — le seul moyen de faire du weak-subjectivity correctement.
3. Imposer HTTPS (rejeter les URL `http://` hors `localhost`) et documenter l'exigence de pinning.
4. Croiser le snapshot avec au moins deux pairs indépendants avant adoption.

### Test permettant de confirmer/réfuter
Servir un snapshot dont `state_root` (champ) est cohérent avec `state_hex` mais différent de
`block.header.state_root`, et asserter le rejet.

---

## VINX-11

**Titre :** La Proof-of-Possession BLS n'est liée ni à l'adresse du validateur ni au chain_id
**Gravité :** MEDIUM
**Confiance :** CONFIRMED
**Fichier :** `crates/vinx-crypto/src/bls.rs`, `crates/vinx-state/src/world_state.rs`
**Fonction :** `BlsSecretKey::proof_of_possession`, `BlsPubKey::verify_pop`, `WorldState::apply_register_bls_key`
**Lignes :** bls.rs 81-86, 100-110 ; world_state.rs 2266-2329

### Description
La PoP est `sign(pk_bytes, DST = BLS_POP_DST)` : elle prouve la possession de la clé, mais rien
d'autre. `apply_register_bls_key` ne vérifie pas que le couple `(bls_pub_key, bls_pop)` n'est pas
déjà enregistré par un autre validateur, et le couple est **stocké en clair dans l'état**, donc
publiquement lisible (`ValidatorPoolEntry::bls_pub_key` / `bls_pop`).

Un validateur B peut donc recopier `(bls_pub_key, bls_pop)` du validateur A depuis l'état et les
enregistrer comme siens. La PoP se vérifie, la transaction est acceptée.

Conséquence sur `bls_signer_count_from_bitmap` : deux index du bitmap pointent alors sur la même
clé G1. `AggregatePublicKey::aggregate` calcule `2·pk_A` et l'agrégat `2·σ_A` — obtenu en
agrégeant **deux fois** la signature publique de A, opération que n'importe qui peut faire.
Le compte de signataires est gonflé sans que A n'ait signé deux fois.

### Preuve dans le code
```rust
// bls.rs:82-86 — la PoP ne couvre que pk_bytes
pub fn proof_of_possession(&self) -> BlsSignature {
    let pk_bytes = self.public_key().0;
    let sig = self.0.sign(&pk_bytes, BLS_POP_DST, &[]);
    BlsSignature(sig.compress())
}
// world_state.rs:2314-2320 — aucun contrôle d'unicité avant écriture
entry.bls_pub_key = Some(payload.bls_pub_key);
entry.bls_pop     = Some(payload.bls_pop);
```

### Scénario d'exploitation
Un attaquant contrôlant k emplacements de validateur y enregistre tous la clé BLS d'un honnête A.
Il attend une co-signature de A sur un bloc, l'agrège k fois et positionne les k bits
correspondants. Le bloc compte k+1 signataires alors qu'un seul a réellement signé le message.
L'attaquant reste plafonné par le nombre d'emplacements bondés qu'il détient — l'attaque ne casse
donc pas le seuil BFT à elle seule, mais elle brise l'hypothèse « un bit du bitmap = une décision
indépendante », ce sur quoi repose la comptabilité de fiabilité (`record_block_cosigns`,
world_state.rs:634-652) et la distribution du pot d'époque (world_state.rs:758-810).

### Impact
Détournement de la distribution proportionnelle du pot d'époque ; corruption des scores de
fiabilité ; hypothèse d'indépendance des co-signatures invalidée.

### Correction recommandée
Signer la PoP sur `bls_pub_key ‖ validator_address ‖ chain_id` et vérifier de même à
l'enregistrement. Rejeter dans `apply_register_bls_key` toute clé G1 déjà présente dans
`validator_pool` sous une autre adresse. Rejeter également, dans
`bls_signer_count_from_bitmap`, tout doublon de clé parmi les signataires reconstruits.

### Test permettant de confirmer/réfuter
Enregistrer la même `bls_pub_key`/`bls_pop` pour deux adresses de validateur distinctes et
asserter que la seconde `RegisterBlsKey` est rejetée.

---

## VINX-12

**Titre :** `Transaction::signing_bytes` ambigu à la frontière `payload` / `sponsor`
**Gravité :** MEDIUM
**Confiance :** CONFIRMED
**Fichier :** `crates/vinx-core/src/transaction.rs`
**Fonction :** `Transaction::signing_bytes`, `Transaction::hash`
**Lignes :** 139-169 (lignes 158-168), 171-173

### Description
`payload` est concaténé **sans préfixe de longueur**, immédiatement suivi du marqueur de sponsor
(`0x00`, ou `0x01 ‖ sponsor(20)`). L'encodage n'est donc pas injectif : pour toute adresse de
sponsor `S` dont le dernier octet vaut `0x00`,

```
payload = P,               sponsor = Some(S)   →  … ‖ P ‖ 0x01 ‖ S[0..20]
payload = P ‖ 0x01 ‖ S[0..19], sponsor = None  →  … ‖ P ‖ 0x01 ‖ S[0..19] ‖ 0x00
```
produisent **exactement les mêmes octets signés**, donc le même `hash()` (le txid ne couvrant
pas la signature). Une signature vaut pour deux transactions sémantiquement différentes.

Le champ `expires_at_height` est, lui, correctement auto-délimité (le drapeau `0x00`/`0x01`
distingue les deux cas) : contrairement à une première hypothèse, il n'y a **pas** de collision
TTL. Le défaut est circonscrit à la frontière payload/sponsor.

### Preuve dans le code
PoC `poc_signing_bytes_payload_sponsor_ambiguity` : `signing_bytes()` et `hash()` sont égaux pour
les deux variantes, `verify_tx_signature_pure` accepte la variante ré-découpée, et le payeur du
fee change (l'émetteur paie au lieu du sponsor).

### Scénario d'exploitation
Probabilité 1/256 par transaction sponsorisée (dernier octet de l'adresse du sponsor). Un relais
hostile transforme la transaction en vol : l'émetteur paie le fee qu'il croyait sponsorisé.
Réciproquement, pour les types dont le payload est décodé (`AdminAction` → `GovernanceAction`,
`SlashValidator` → `SlashEvidence`), un ré-découpage tronque le payload de 20 octets et modifie
la sémantique décodée sous une signature valide — non démontré ici faute d'un cadrage bincode
exploitable, mais la propriété d'injectivité est perdue.

### Impact
Malléabilité sémantique de transactions signées. Deux transactions distinctes partagent un txid,
ce qui casse également les invariants de déduplication (mempool `seen`, `Chain::tx_index`).

### Correction recommandée
Préfixer `payload` de sa longueur (`u32` big-endian) dans `signing_bytes`, et le faire
inconditionnellement (y compris quand il est vide, pour éviter une seconde ambiguïté).
Étendre `test_signing_bytes_golden_vector` avec un vecteur payload non vide + sponsor. Faire
inclure au `hash()` un domaine de séparation (`b"VINX_TX_V1"`), absent aujourd'hui.
**Changement cassant** : coordonner avec le SDK TypeScript et `rpc/ui.rs`.

### Test permettant de confirmer/réfuter
Le PoC ci-dessus, à inverser (`assert_ne!`) après correctif.

---

## VINX-13

**Titre :** `Transaction::payload` n'est borné nulle part
**Gravité :** MEDIUM
**Confiance :** CONFIRMED
**Fichier :** `crates/vinx-core/src/transaction.rs`, `crates/vinx-state/src/world_state.rs`
**Fonction :** `WorldState::admission_check`, `WorldState::apply_transfer`
**Lignes :** world_state.rs 1180-1247, 1389-1494

### Description
Aucun contrôle de taille de `payload` n'existe : ni à l'admission mempool, ni à l'application,
ni pour les types qui ne s'en servent pas (`Transfer`, `Stake`, `Unstake`, `Unjail`). Le fee est
un forfait plat indépendant de la taille (`Amount::calculate_fee` retourne `base_fee`,
amount.rs:255-257).

### Preuve dans le code
PoC `poc_unbounded_payload_accepted` : un `Transfer` portant 4 MiB de `payload` passe
`admission_check` puis `apply_transaction` pour le fee plancher (0,0001 VINX).

### Scénario d'exploitation
Un attaquant remplit les blocs de transactions de plusieurs mégaoctets au prix plancher. Les
blocs sont gossipés (jusqu'à `MAX_DECODED_BYTES = 16 MiB`), persistés, servis en sync, et
conservés `TX_RETENTION_SECS = 90 jours`. Le mempool accepte 100 000 entrées — soit un plafond
théorique de plusieurs centaines de gigaoctets en RAM.

### Impact
Gonflement du stockage et de la bande passante à coût quasi nul ; épuisement mémoire du mempool.

### Correction recommandée
Introduire `MAX_TX_PAYLOAD_BYTES` (par ex. 4 KiB, largement au-dessus des besoins :
`AnnounceUpgrade` 14 o, `RegisterVrfKey` 32 o, `RegisterBlsKey` ~160 o, `SlashEvidence` ~400 o) et
le contrôler dans `admission_check` **et** dans `dispatch_tx`. Exiger `payload.is_empty()` pour
`Transfer`/`Stake`/`Unstake`/`Unjail`. Tarifer au-delà d'un seuil (fee proportionnel à la taille).

### Test permettant de confirmer/réfuter
Le PoC ci-dessus, à inverser après correctif.

---

## VINX-14

**Titre :** `GovernanceAction::ScheduleUpgrade` contourne le délai de préavis de l'ADR 0006
**Gravité :** MEDIUM
**Confiance :** CONFIRMED
**Fichier :** `crates/vinx-state/src/world_state.rs`
**Fonction :** `WorldState::execute_governance_action` (bras `ScheduleUpgrade`) vs `WorldState::apply_announce_upgrade`
**Lignes :** 2026-2040 (gouvernance, sans contrôle) ; 1627-1686 (contrôle présent, lignes 1650-1660)

### Description
`apply_announce_upgrade` impose le préavis réglementaire :
```rust
// world_state.rs:1655-1660
if activation_ts < announcement_ts.saturating_add(min_notice) {
    return Err(CoreError::UpgradeViolation(...));
}
```
(7 jours pour un patch, 30 pour un mineur, 90 pour un majeur.)

Le chemin de gouvernance ne le fait pas :
```rust
// world_state.rs:2029-2039
GovernanceAction::ScheduleUpgrade { version, activation_ts } => {
    if self.pending_upgrade.is_none() {
        self.pending_upgrade = Some(ScheduledUpgrade { version, activation_ts, announced_at: self.current_block_ts });
    }
}
```
Aucune validation de `activation_ts`, aucune de la cohérence de `version` (une régression de
version est acceptée). De plus, l'échec silencieux (`if is_none()` sans `else { Err }`) fait
consommer le nonce et retourner `Ok(())` alors que rien n'a été fait.

`check_upgrade_activation` (world_state.rs:1688-1706) active dès
`current_block_ts >= activation_ts` : un `activation_ts` dans le passé active au bloc suivant.

### Preuve dans le code
Voir extraits. Le contraste entre les deux chemins est direct.

### Scénario d'exploitation
Une clé admin compromise (ou un comité atteignant son seuil) planifie une mise à niveau majeure
avec `activation_ts = 0`. Elle s'active au bloc suivant, sans les 90 jours de préavis que l'ADR
0006 promet à l'écosystème pour réagir.

### Impact
Contournement du principal garde-fou temporel de la gouvernance ; les opérateurs n'ont aucune
fenêtre pour refuser une mise à niveau hostile.

### Correction recommandée
Extraire le contrôle de préavis dans une fonction `validate_upgrade_schedule(&self, version,
activation_ts) -> Result<()>` appelée par les deux chemins. Retourner une erreur explicite
lorsqu'une mise à niveau est déjà en attente, au lieu du no-op silencieux. Refuser une version
inférieure ou égale à la version courante.

### Test permettant de confirmer/réfuter
`AdminAction(ScheduleUpgrade { version: 2.0.0, activation_ts: current_block_ts })` doit être
rejeté. Aujourd'hui il est accepté.

---

## VINX-15

**Titre :** Les clés privées du nœud sont écrites en clair avec les permissions par défaut
**Gravité :** MEDIUM
**Confiance :** CONFIRMED
**Fichier :** `crates/vinx-node/src/main.rs`
**Fonction :** `KeyFile::load_or_generate`
**Lignes :** 107-121 (ligne 118 : `std::fs::write(path, json)`)

### Description
Les fichiers de clés du nœud (validateur, admin, faucet) contiennent `secret_key_hex` en clair et
sont créés par `std::fs::write`, donc avec le mode par défaut soumis à l'umask — typiquement
`0644`, lisible par tout utilisateur local. Aucun chiffrement, aucun `set_permissions`.

Le wallet, lui, fait les choses correctement : `keystore::write_private` applique `0o600`
(keystore.rs:267-277) et propose Argon2id + AES-256-GCM.

### Preuve dans le code
```rust
// main.rs:116-118
let json = serde_json::to_string_pretty(&kf).unwrap();
std::fs::write(path, json).expect("write key file");
```
Aucune occurrence de `PermissionsExt` dans `crates/vinx-node/src`.

### Scénario d'exploitation
Sur un hôte partagé, un conteneur avec volume monté, ou une image Docker construite avec la clé,
tout processus non privilégié lit la clé du validateur. Elle permet de produire des blocs à sa
place, de signer des transactions depuis son compte (dont son bond de 100 000 VINX) et, via la
dérivation `sha256(validator_secret)` (p2p/mod.rs:127-140), d'usurper son identité libp2p.

### Impact
Compromission complète d'un validateur par un accès local non privilégié.

### Correction recommandée
Réutiliser `vinx_wallet::keystore::write_private` (ou dupliquer sa logique) : `0o600` sur Unix,
création via `OpenOptions::new().mode(0o600).create_new(true)` pour éviter la fenêtre TOCTOU
entre `write` et `chmod`. Refuser au démarrage de charger un fichier de clé dont le mode est plus
permissif que `0600`. Proposer le chiffrement par passphrase (`VINX_NODE_PASSPHRASE`) comme le
wallet.

### Test permettant de confirmer/réfuter
Générer un fichier de clé de nœud et asserter `mode & 0o777 == 0o600`, à l'image de
`keystore::tests::test_file_permissions_0600`.

---

## VINX-16

**Titre :** Candidats de fork-choice non bornés en mémoire
**Gravité :** MEDIUM
**Confiance :** LIKELY
**Fichier :** `crates/vinx-node/src/chain.rs`, `crates/vinx-node/src/p2p/mod.rs`
**Fonction :** `Chain::record_candidate`, `consider_competing_block`
**Lignes :** chain.rs 206-225 ; p2p/mod.rs 483-518

### Description
`record_candidate` range tout bloc concurrent valide dans `candidates: AHashMap<u64, Vec<Block>>`,
dédupliqué par hash seulement. Aucun plafond n'existe : ni par hauteur, ni global. La purge
(`prune_candidates_final`, chain.rs:292-296) n'intervient que lorsque `finalized_height` avance —
c'est-à-dire jamais pendant une attaque qui bloque la finalité.

Un bloc est facilement rendu unique : il suffit d'incrémenter `timestamp`. Chaque bloc peut peser
jusqu'à plusieurs mégaoctets (VINX-13).

### Preuve dans le code
```rust
// chain.rs:218-224
let bucket = self.candidates.entry(h).or_default();
if bucket.iter().any(|b| b.hash() == hash) { return false; }
bucket.push(block);   // aucune borne
```
De plus, `bucket.iter().any(|b| b.hash() == hash)` recalcule le hash de chaque candidat déjà
stocké : le coût d'insertion est quadratique en nombre de candidats.

### Scénario d'exploitation
Combiné à VINX-01 (aucune signature exigée) : un attaquant publie des milliers de blocs valides
distincts à la hauteur du tip. Chacun est cloné dans `candidates` et déclenche un
`rebuild_canonical_state` si le fork-choice le désigne. Mémoire et CPU croissent sans limite.

### Impact
Épuisement mémoire et CPU du nœud. Marqué LIKELY : le déclenchement dépend de VINX-01 ou de la
possession d'une clé de validateur.

### Correction recommandée
Plafonner `candidates` par hauteur (`MAX_CANDIDATES_PER_HEIGHT`, par ex. 4, en ne conservant que
les plus canoniques) et globalement, et purger toute hauteur `< tip - MAX_UNFINALIZED_DEPTH`.
Remplacer la déduplication linéaire par un `AHashSet<Hash32>`.

### Test permettant de confirmer/réfuter
Injecter 10 000 candidats distincts à une hauteur et asserter que `candidates_at(h).len()` reste
plafonné.

---

## VINX-17

**Titre :** ECVRF sans `validate_key` RFC 9381 ; beacon d'époque dépourvu d'entropie
**Gravité :** MEDIUM
**Confiance :** CONFIRMED
**Fichier :** `crates/vinx-crypto/src/vrf.rs`, `crates/vinx-state/src/world_state.rs`
**Fonction :** `vrf::verify`, `WorldState::apply_register_vrf_key`, `WorldState::tick_epoch_close`
**Lignes :** vrf.rs 201-240 ; world_state.rs 2334-2385 (2360-2371) ; world_state.rs 822-830

### Description
Trois défauts distincts :

1. **`validate_key` manquante.** RFC 9381 §5.4.5 impose de rejeter une clé publique `Y` telle que
   `cofactor·Y` est l'identité. `vrf::verify` (vrf.rs:221-223) se contente de décompresser `Y`.
   Avec `Y = identité` et `Γ = identité`, `U = s·B` et `V = s·H` pour tout `s`, donc `c = c'`
   trivialement : **n'importe quelle preuve se vérifie sans connaître de clé secrète.** La sortie
   β est alors constante (hash de l'identité), donc non grindable — mais le contrôle
   d'authenticité est nul.

2. **Aucune validation à l'enregistrement.** `apply_register_vrf_key` contient un contrôle vide
   (`if VrfPublicKey(key_bytes).0.len() != 32`, toujours faux) et un commentaire admettant que
   les octets sont acceptés tels quels (world_state.rs:2360-2371). Une clé d'ordre faible entre
   donc dans l'état.

3. **Beacon sans entropie.** `epoch_beacon' = sha256(beacon ‖ epoch_number ‖ block_ts)`
   (world_state.rs:822-830). Aucune contribution de validateur, aucune sortie VRF. Le beacon est
   entièrement prédictible par quiconque connaît les timestamps futurs, et le producteur du bloc
   de clôture d'époque en choisit partiellement `block_ts`.

### Preuve dans le code
```rust
// vrf.rs:221-223 — pas de contrôle d'ordre faible
let y = CompressedEdwardsY(pk.0).decompress().ok_or(CryptoError::InvalidSignature)?;
// world_state.rs:2364-2371
if VrfPublicKey(key_bytes).0.len() != 32 { ... }   // toujours faux
// « just accept the 32 bytes — they are validated on first use » (ils ne le sont pas)
```

### Scénario d'exploitation
Un validateur enregistre l'identité Edwards25519 comme clé VRF. Toute preuve qu'il soumet
(y compris fabriquée sans secret) se vérifie. Comme la sélection de comité n'est pas encore
câblée (VINX-18), l'impact est aujourd'hui limité au bruit ; il devient critique dès l'activation.

### Impact
Le mécanisme de randomisation du protocole n'offre aucune garantie : preuves VRF forgeables et
beacon prédictible. Toute logique future de sélection de comité, de leader ou de loterie bâtie
dessus serait entièrement contrôlable.

### Correction recommandée
1. Implémenter `ECVRF_validate_key` : rejeter `Y` tel que `Y.mul_by_cofactor().is_identity()`,
   à la vérification **et** à l'enregistrement (`apply_register_vrf_key` doit décompresser le
   point et refuser l'ordre faible).
2. Rejeter également `Γ` d'ordre faible dans `verify` et `proof_to_hash`.
3. Remplacer le beacon par un accumulateur des sorties VRF des validateurs de l'époque
   (`beacon' = sha256(beacon ‖ Σ β_i triés)`), afin qu'aucune partie ne puisse le prédire seule.
4. Ajouter les vecteurs de test RFC 9381 (suite 0x03) au crate crypto — ils sont absents.

### Test permettant de confirmer/réfuter
`verify(&VrfPublicKey(identity_bytes), &VrfProof(forged), alpha)` doit échouer. Aujourd'hui, avec
`Γ = identité` et un `s` arbitraire, il réussit.

---

## VINX-18

**Titre :** Sélection de comité VRF non câblée ; classement de repli indépendant du beacon
**Gravité :** MEDIUM
**Confiance :** CONFIRMED
**Fichier :** `crates/vinx-state/src/world_state.rs`, `crates/vinx-node/src/p2p/mod.rs`
**Fonction :** `WorldState::committee_for_height`, `WorldState::committee_from_vrf_proofs`
**Lignes :** world_state.rs 862-899 (865-878), 926-949 ; p2p/mod.rs 1100-1112

### Description
Deux problèmes.

**(a) Le comité n'est utilisé nulle part.** `committee_from_vrf_proofs` n'est appelée qu'en
p2p/mod.rs:1108, et son résultat est uniquement journalisé (`info!(... "VRF committee derived")`).
`committee_for_height` n'a aucun appelant hors tests. Toute la machinerie ADR 0029 Phase 2b est
inerte : la documentation décrit une protection qui n'est pas appliquée.

**(b) Le classement des validateurs VRF ignore le beacon et la hauteur.** Dans
`committee_for_height` :
```rust
// world_state.rs:869-878
let mut buf = [0u8; 21];
buf[..20].copy_from_slice(addr.as_bytes());
buf[20] = 0xFF;
let mut score = sha256(&buf);
score[0] = 0x00;
```
Le score d'un validateur disposant d'une clé VRF est `sha256(adresse ‖ 0xFF)` — **constant, sans
beacon ni hauteur**. Un attaquant peut donc grinder des paires de clés Ed25519 jusqu'à obtenir une
adresse dont le score est minimal, et occuper le comité **à toutes les hauteurs, pour toujours**.
Le repli SHA-256 (branche `else`) est correct, lui, puisqu'il mélange `beacon ‖ height ‖ addr`.

### Preuve dans le code
Voir extrait et `grep committee_` : aucun appelant de production hors journalisation.

### Impact
Aujourd'hui : documentation trompeuse, faux sentiment de sécurité, code mort dans un chemin
consensus. Au câblage : capture permanente du comité par grinding d'adresses, à coût de calcul
modeste (≈2⁴⁰ hashes pour 5 octets de préfixe).

### Correction recommandée
Soit retirer la machinerie du code de production jusqu'à ce qu'elle soit conçue et câblée, soit
la corriger : le score de repli d'un validateur VRF doit lui aussi dériver de
`sha256(beacon ‖ height ‖ addr)`, et un validateur qui ne fournit pas de preuve VRF ne doit pas
être classé devant ceux qui en fournissent. Documenter clairement, dans l'ADR 0029, que la
sélection n'est pas active.

### Test permettant de confirmer/réfuter
`committee_for_height(1, k)` et `committee_for_height(2, k)` doivent différer pour un pool
composé uniquement de validateurs à clé VRF. Aujourd'hui elles sont identiques.

---

## VINX-19

**Titre :** L'interface web manipule la clé privée sous un script CDN sans SRI ni CSP
**Gravité :** MEDIUM
**Confiance :** CONFIRMED
**Fichier :** `crates/vinx-node/src/rpc/ui.rs`
**Fonction :** page `index`, page `admin`
**Lignes :** 19 et 891 (balise `<script src>`), 576-580 et 1086 (import de clé), 686 et 1140 (signature)

### Description
```html
<script src="https://cdn.jsdelivr.net/npm/tweetnacl@1.0.3/nacl-fast.min.js"></script>
```
Aucun attribut `integrity`, aucun `crossorigin`, et aucun en-tête `Content-Security-Policy` n'est
émis par le routeur (`rpc/mod.rs:16-53`). Cette même page charge la clé privée de l'utilisateur
depuis un fichier JSON (`json.secret_key_hex`, ui.rs:576-577), en dérive une paire nacl
(ui.rs:579) et la garde en mémoire (`wallet.secretKey64`) pour signer (ui.rs:686).

Toute la surface d'affichage utilise `innerHTML` avec des littéraux de gabarit non échappés
(≈40 occurrences), y compris pour des messages d'erreur reflétant l'entrée utilisateur
(ui.rs:554, 564, 674, 700, 724).

### Preuve dans le code
Voir extraits ; `grep -n "integrity=\|Content-Security-Policy" crates/vinx-node/src/rpc/` ne
retourne rien.

### Scénario d'exploitation
Compromission du CDN, empoisonnement DNS, ou simple interception si le nœud est servi en HTTP :
le script substitué exfiltre `wallet.secretKey64` vers un serveur tiers. L'utilisateur ne voit
rien.

### Impact
Vol de la clé privée de tout utilisateur du portefeuille web intégré au nœud, donc de la
totalité de ses fonds.

### Correction recommandée
1. Vendoriser `tweetnacl` dans le binaire et le servir depuis le nœud (`/static/nacl.js`) —
   la dépendance CDN n'a aucune raison d'être pour un logiciel qui embarque déjà ses assets.
2. À défaut, ajouter `integrity="sha384-…"` et `crossorigin="anonymous"`.
3. Émettre `Content-Security-Policy: default-src 'self'; script-src 'self'; connect-src 'self'`
   via une couche axum, plus `X-Content-Type-Options: nosniff` et `Referrer-Policy: no-referrer`.
4. Remplacer les `innerHTML` par `textContent` / création de nœuds, ou introduire un `esc()`
   systématique.

### Test permettant de confirmer/réfuter
Requête sur `/` : asserter la présence de l'en-tête CSP et l'absence de `src="https://`.

---

## VINX-20

**Titre :** Autorité de gouvernance « fail-open » lorsqu'aucun admin n'est configuré
**Gravité :** LOW
**Confiance :** CONFIRMED
**Fichier :** `crates/vinx-state/src/world_state.rs`
**Fonction :** `WorldState::effective_admin`, `WorldState::apply_admin_action`, `WorldState::check_admin`
**Lignes :** 1920-1931 (1927-1929), 1869-1918, 1374-1387

### Description
```rust
// world_state.rs:1926-1930
} else if let Some(admin) = self.admin_address {
    (Some(vec![admin]), 1)
} else {
    (None, 1)          // « dev mode » : aucune restriction
}
```
Quand `signers` vaut `None`, `apply_admin_action` n'effectue **aucun** contrôle d'autorisation et
exécute l'action avec `threshold = 1`. `check_admin` fait de même (world_state.rs:1379-1385).
Un état où `admin_address` et `admin_policy` sont tous deux `None` accorde donc les pleins
pouvoirs de gouvernance — ajout/retrait de validateurs, rotation d'admin, planification de mise à
niveau — à **n'importe quelle adresse**.

`create_genesis_state` renseigne toujours `admin_address`, mais `WorldState::new()` ne le fait pas,
et `POST /snapshot` ou une désérialisation bincode peuvent produire cet état.

### Preuve dans le code
Voir extrait, plus `WorldState::new()` (world_state.rs:423 : `admin_address: None`).

### Impact
Mode d'échec ouvert sur un chemin de sécurité. Ne devient exploitable que via un état importé ou
un chemin de construction non-genesis, d'où la gravité LOW.

### Correction recommandée
Inverser le défaut : sans admin ni comité, **refuser** toute `AdminAction` (`Err(Unauthorized)`).
Réserver le mode permissif à un drapeau explicite `#[cfg(feature = "dev-open-governance")]` ou à
un champ de configuration nommé sans ambiguïté, jamais au `None` par défaut.

### Test permettant de confirmer/réfuter
Sur un `WorldState::new()`, une `AdminAction(AddValidator)` signée par une adresse quelconque doit
être rejetée. Aujourd'hui elle est exécutée.

---

## VINX-21

**Titre :** Réputation des pairs monotone décroissante — bannissement définitif de pairs honnêtes
**Gravité :** LOW
**Confiance :** CONFIRMED
**Fichier :** `crates/vinx-node/src/p2p/guard.rs`
**Fonction :** `PeerGuard::penalize`, `PeerGuard::admit`
**Lignes :** 112-116, 97-108

### Description
La réputation ne remonte jamais : `penalize` ne fait que soustraire, et rien ne la restaure.
`BAN_THRESHOLD = -5` avec `RATE_FLOOD_PENALTY = 1` signifie que **cinq dépassements de débit sur
toute la durée de vie de la connexion** suffisent à blacklister définitivement un pair. Le seuil
de 50 msg/s est atteint naturellement lors d'une rafale de co-signatures sur un grand set de
validateurs, ou lors d'un rattrapage de sync.

### Preuve dans le code
```rust
// guard.rs:112-116
pub fn penalize(&mut self, peer: PeerId, points: i32) -> bool {
    let score = self.reputation.entry(peer).or_insert(0);
    *score -= points;                 // jamais d'incrément
    *score <= BAN_THRESHOLD
}
```
`forget` (guard.rs:119-122) efface l'état à la déconnexion, ce qui a l'effet pervers inverse :
un attaquant qui se reconnecte repart d'une réputation neuve.

### Impact
Partition progressive du réseau : les pairs les plus actifs (donc les validateurs) se bannissent
mutuellement au fil du temps. À l'inverse, aucune persistance ne pénalise un attaquant qui
recycle ses connexions.

### Correction recommandée
Faire décroître la pénalité dans le temps (récupération d'un point par minute, plafonnée à 0), et
conserver la réputation des pairs bannis au-delà de la déconnexion (LRU borné) pour que le
recyclage de connexion ne réinitialise pas le score.

### Test permettant de confirmer/réfuter
Simuler 6 rafales espacées d'une heure et asserter que le pair n'est pas banni.

---

## VINX-22

**Titre :** Structures non bornées : `faucet_cooldowns`, `Mempool::seen`, index de transactions
**Gravité :** LOW
**Confiance :** LIKELY
**Fichier :** `crates/vinx-node/src/node.rs`, `crates/vinx-node/src/mempool.rs`, `crates/vinx-node/src/chain.rs`
**Fonction :** `faucet_request`, `Mempool::add`, `Chain::push`
**Lignes :** node.rs 131, 177, 253 ; handlers.rs 715-720 ; mempool.rs 128 ; chain.rs 410-420

### Description
* `faucet_cooldowns: Arc<Mutex<HashMap<Address, Instant>>>` : une entrée est insérée par adresse
  servie et **n'est jamais purgée**, même après expiration du cooldown (24 h par défaut).
* `Mempool::seen` : purgé sur `drain`, `prune_expired` et `update_confirmed_nonces`, mais pas sur
  le chemin `stage`/`flush_staged` en cas de rejet. Le filtre de Bloom est dimensionné pour
  `max_size * 2` et n'est reconstruit que sur suppression.
* `Chain::account_tx_index: AHashMap<Address, Vec<Hash32>>` : croît indéfiniment ; `prune_by_age`
  et `prune` le reconstruisent à partir des blocs restants, ce qui borne la croissance mais
  impose un `rebuild_tx_index()` O(blocs × tx) à chaque passe.

### Impact
Croissance mémoire lente sur les nœuds à faucet public ; pics de latence lors des reconstructions
d'index. Pas d'exploitation directe.

### Correction recommandée
Remplacer `faucet_cooldowns` par un `LruCache` borné, ou purger les entrées expirées à chaque
requête. Documenter et tester le coût de `rebuild_tx_index` à l'échelle cible.

### Test permettant de confirmer/réfuter
10 000 requêtes faucet depuis 10 000 adresses distinctes, puis mesurer la taille de la map après
expiration du cooldown.

---

## VINX-23

**Titre :** `Mempool::try_evict_for` scanne tout le mempool à chaque insertion quand il est plein
**Gravité :** LOW
**Confiance :** CONFIRMED
**Fichier :** `crates/vinx-node/src/mempool.rs`
**Fonction :** `Mempool::try_evict_for`
**Lignes :** 296-325

### Description
```rust
for (addr, queue) in &self.queues {
    for (&nonce, tx) in queue { if tx.fee.atoms() < min_fee { ... } }
}
```
Double boucle sur les 100 000 entrées, exécutée **sous le verrou d'écriture du mempool**, pour
chaque insertion tentée dès que `pending_count >= max_size`.

### Impact
Un attaquant maintient le mempool plein (au fee plancher) puis soumet des transactions à faible
fee : chaque soumission coûte un balayage complet. Amplification ~10⁵ par requête, sous verrou —
la production de blocs et le RPC sont ralentis en même temps.

### Correction recommandée
Maintenir un tas min (`BinaryHeap<Reverse<(u128, Address, u64)>>`) des fees en parallèle des
files, ou une file de priorité indexée, pour une éviction en O(log n).

### Test permettant de confirmer/réfuter
Benchmark : mempool plein à 100 000, mesurer le débit de `add()` — attendu O(log n) après
correctif.

---

## VINX-24

**Titre :** Ed25519 : `verify` non strict — clés et signatures d'ordre faible acceptées
**Gravité :** INFO
**Confiance :** CONFIRMED
**Fichier :** `crates/vinx-crypto/src/keys.rs`
**Fonction :** `PublicKey::verify`
**Lignes :** 67-73 (ligne 71 : `vk.verify(message, &sig)`)

### Description
`ed25519-dalek 2.x` expose `verify` (permissif, conforme à RFC 8032 mais acceptant les points
d'ordre faible et les clés de torsion) et `verify_strict` (rejette les clés/`R` d'ordre faible,
donc garantit la non-malléabilité des identités). VinX utilise `verify`.

La malléabilité du scalaire `S` est bien bloquée (dalek 2.x rejette un `S` non canonique lors de
la conversion interne), et le `hash()` d'une transaction ne couvre pas la signature — la
malléabilité de txid n'est donc pas exploitable par ce biais.

Il reste que, l'adresse étant `sha256(pubkey)[..20]`, une clé d'ordre faible produit une adresse
valide dont les signatures peuvent vérifier sous plusieurs clés. Aucun scénario concret d'atteinte
aux fonds n'a été identifié dans ce code ; le classement est INFO.

### Correction recommandée
Utiliser `verify_strict` — coût nul, élimine une classe entière d'ambiguïtés d'identité.

### Test permettant de confirmer/réfuter
Construire une clé d'ordre faible et une signature valide sous cette clé, asserter le rejet.

---

# Synthèse

## 1. Les cinq risques les plus importants

1. **VINX-01 + VINX-02 : la chaîne n'authentifie pas ses blocs.** Le chemin gossip ne vérifie
   aucune signature de bloc, l'en-tête ne porte pas de signature de proposeur, et le seul contrôle
   de quorum existant se désactive en omettant un champ. Un attaquant sans clé fabrique et
   finalise des blocs. C'est une rupture totale de la sûreté du consensus, pas une faiblesse
   graduelle.
2. **VINX-03 : vol de fonds arbitraire via le champ `sponsor`.** Une signature manquante n'est
   jamais vérifiée sur le chemin qui mène à l'application, et le fee n'est pas plafonné. N'importe
   quel solde se vide avec la seule connaissance d'une adresse. Effet de bord : arrêt de la
   production de blocs du réseau.
3. **VINX-04 : le `state_root` n'engage pas l'état de consensus.** Set de validateurs, registre
   BLS, autorité admin, beacon, bonds : rien n'est couvert. Tout fork sur ces champs est
   silencieux, et le contrôle de racine du snapshot-sync ne prouve rien de ce qui compte.
4. **VINX-06 : un seul validateur byzantin capture la production.** Trois tours de pré-emption
   suffisent à emprisonner chaque honnête ; la rotation ne contient plus que l'attaquant.
5. **VINX-08 + VINX-09 : deux DoS distants à un message.** Une panique déterministe sur les nœuds
   amorcés par snapshot (le cas de tout nouveau nœud d'une chaîne mature), et un travail
   quadratique non borné sur `CompactBlock`.

## 2. Les parties du protocole qui demandent le plus d'attention

**Le chemin d'acceptation de bloc, en priorité absolue.** Il existe aujourd'hui **quatre**
implémentations divergentes de « valider un bloc » : `p2p::NewBlock`, `p2p::SyncResponse`,
`sync::sync_from_peer` et `sync::parallel_sync_from_peer`, plus deux chemins compacts qui
court-circuitent tout. Elles ne font pas les mêmes contrôles (VINX-01, VINX-07). C'est la cause
racine de la moitié des findings critiques. Il faut **une seule** fonction
`validate_and_apply_block(&mut state, &mut chain, &block, source) -> Result<()>` appelée par les
six entrées, sans exception.

**Le modèle de commitment.** `state_root` doit engager tout l'état de consensus (VINX-04), et
`signing_bytes` doit devenir injectif avec séparation de domaine (VINX-12). Ce sont des
changements cassant le consensus : à faire maintenant, pas après un lancement.

**La duplication des vérifications de signature.** `verify_tx_signature_pure` est la référence,
mais trois copies partielles existent (`handlers::submit_tx`, `handlers::submit_tx_batch`,
`Mempool::verify_sig_static`) et c'est exactement là qu'est née VINX-03. Il ne doit rester
qu'un appelant.

**La frontière entre « vérifié » et « de confiance ».** `apply_transaction_trusted` est une
fonction dangereuse dont la précondition n'est vérifiée nulle part. Elle mérite un type
`VerifiedTransaction` que seule la vérification complète peut construire — la sûreté serait alors
garantie par le compilateur plutôt que par un commentaire.

**Le module ECVRF et la sélection de comité (ADR 0029).** Environ 500 lignes de code cryptographique
non conforme à RFC 9381, sans vecteurs de test, non câblées, et décrites dans la documentation
comme une protection active. Soit le corriger et le câbler, soit le retirer.

**La comptabilité de fiabilité (ADR 0027).** Attribuer un manquement sans preuve de créneau écoulé
est un vecteur de grief direct (VINX-06).

## 3. Hypothèses que je n'ai pas pu vérifier

* **VINX-07 (LIKELY)** : le saut d'horloge protocole via `SyncResponse` est déduit du code
  (contrôle de dérive absent, MTP sur 11 blocs, courbe d'émission fonction du temps écoulé) mais
  n'a pas été exécuté de bout en bout — cela demande un harnais multi-nœuds que je n'ai pas monté.
* **VINX-16 (LIKELY)** : la croissance non bornée de `candidates` est claire dans le code ; je n'ai
  pas mesuré le seuil réel d'épuisement mémoire.
* **Ordre des verrous.** `tick`, `consider_competing_block` et les bras P2P prennent
  `state → chain → validator_set → finalized_state` de manière apparemment cohérente, mais je n'ai
  pas prouvé l'absence d'interblocage sous charge. Aucun test de concurrence n'existe. À traiter
  par un ordre de verrouillage documenté et un test sous `loom` ou stress multi-thread.
* **Migrations de stockage (`storage.rs` v7→v18).** La stratégie « ajout en suffixe bincode »
  repose sur le fait que les nouveaux champs restent les derniers champs *sérialisés* de
  `WorldState`. Je n'ai pas vérifié chaque migration ; une erreur d'ordre corromprait silencieusement
  l'état au chargement. Cette contrainte devrait être verrouillée par un test golden par version.
* **Déterminisme inter-plateformes.** Aucun flottant n'apparaît dans la transition d'état (bon
  point), mais `update_base_fee` et la distribution du pot d'époque font des divisions entières dont
  je n'ai pas audité tous les cas limites. `HashMap` (non ordonné) est utilisé dans `apply_transfer`
  pour les deltas ED — l'itération est non déterministe, mais la boucle ne fait que des contrôles
  (aucune mutation), donc le résultat l'est. À surveiller si ce code évolue.
* **SDK TypeScript et signeur navigateur.** Non audités (hors `rpc/ui.rs`). Toute divergence avec
  `signing_bytes` produirait des signatures invalides ou, pire, exploiterait VINX-12.
* **`crates/vinx-desktop-core` et l'application Tauri.** Non audités faute de temps ; ils manipulent
  des clés et méritent une passe dédiée.

## 4. Tests de sécurité à ajouter

**Consensus / blocs**
1. Un bloc dont `bls_bitmap` est vide est rejeté sur les six chemins d'ingestion.
2. Un bloc signé par des clés BLS non enregistrées est rejeté, quel que soit leur nombre.
3. Un bloc dont `header.validator` est un validateur honnête mais qui n'est signé par personne est
   rejeté (test anti-usurpation de proposeur).
4. `canonical_head` élit le bloc à 2 co-signataires enregistrés plutôt que celui à 50 clés inconnues.
5. Une réorg sur une chaîne `height_base = 1000` réussit sans panique ; test paramétré
   `height_base ∈ {0, 1000}` sur toutes les méthodes de `Chain`.
6. Un `SyncResponse` de 6 blocs à `now + 10 ans` ne modifie ni `emitted_atoms` ni les
   `pending_unbonds`.
7. Fuzzing de `P2pMessage::decode` + `dispatch_message` (cargo-fuzz sur le décodeur borné puis sur
   chaque bras), avec assertion « aucune panique ».

**Transactions / état**
8. Une transaction sponsorisée sans `sponsor_signature` valide est rejetée par `submit_tx`,
   `submit_tx_batch`, `flush_staged` **et** `admission_check`.
9. Invariant d'injectivité : `signing_bytes` distinct pour toute paire de transactions distinctes
   (test de propriété sur `payload`, `sponsor`, `expires_at_height`).
10. Test de propriété : `hash(tx1) == hash(tx2) ⟹ tx1 == tx2` (hors signature).
11. `payload` au-delà de `MAX_TX_PAYLOAD_BYTES` rejeté ; `payload` non vide rejeté pour
    `Transfer`/`Stake`/`Unstake`/`Unjail`.
12. Test de propriété sur l'invariant de masse : après une séquence arbitraire de transactions
    valides, `circulating + pot + destroyed == emitted ≤ MAX_SUPPLY`. (Le fichier
    `crates/vinx-state/tests/property_tests.rs` existe — l'étendre au slashing, au pot d'époque
    et aux réorgs.)
13. `state_root` change lors de la mutation de **chacun** des champs de consensus (un test par
    champ, à écrire après VINX-04).

**Gouvernance / validateurs**
14. `ScheduleUpgrade` par gouvernance respecte le préavis ADR 0006.
15. La même clé BLS ne peut pas être enregistrée par deux validateurs.
16. Aucune `AdminAction` n'est exécutable sans admin ni comité configuré.
17. Un attaquant proposant systématiquement hors de son créneau ne jaile aucun honnête.

**Cryptographie**
18. Vecteurs de test RFC 9381 (suite ECVRF-EDWARDS25519-SHA512-TAI) pour `prove` / `verify` /
    `proof_to_hash`.
19. Clé VRF d'ordre faible : rejetée à l'enregistrement et à la vérification.
20. Vecteurs de test RFC 8032 pour Ed25519, dont les cas d'ordre faible de la suite de Chalkias
    (« Taming the many EdDSAs »).
21. Vecteurs BLS12-381 (draft-irtf-cfrg-bls-signature) pour `sign`/`verify`/`aggregate`, et un
    test de rejet du rogue-key sans PoP.

**Ressources / DoS**
22. Benchmark `CompactBlock` : 100 000 hashes inconnus contre un mempool de 50 000 → borne de temps.
23. Benchmark `Mempool::add` sur mempool plein → complexité logarithmique.
24. `candidates_at(h).len()` reste plafonné après 10 000 injections.
25. Permissions `0600` sur tous les fichiers de clés créés par le nœud.
26. Présence de l'en-tête CSP et absence de `<script src="https://` sur `/` et `/admin`.
