# CLAUDE.md : contexte de travail sur VinX Ledger

À lire en début de session. Pour les règles exactes du protocole, voir
`SPEC.md` ; pour le modèle du réseau, `docs/VINX-CONSENSUS.md`.

## 1. Règles de travail avec le propriétaire (Haito)

- **Tout en français**, avec un guidage pas à pas et des commandes prêtes à
  copier pour ses serveurs.
- **Demander validation avant de confirmer ou de lancer quoi que ce soit** :
  proposer, attendre son « oui ».
- **Être objectif**, sans complaisance : dire franchement ce qui ne va pas,
  ce qui est risqué, ce qui n'est pas prouvé.
- **Fusionner soi-même une PR quand la CI est entièrement verte** (squash),
  puis se désabonner et supprimer le rappel programmé.
- Après une fusion qui change un binaire : lui dire de **publier la release**
  `vX.Y.Z` (GitHub, Releases, Draft, tag sur `main`) et quels nœuds mettre à
  jour.
- Ne jamais présenter un résultat non vérifié comme acquis. Signaler ce qui
  n'a pas pu être testé (APK sur téléphone, signature en CI…).

## 2. Le projet

Blockchain de paiement en Rust, sans contrats intelligents : toutes les
fonctions sont **natives**.

- **Fonctions** : paiements, noms `.vinx`, séquestres, mandats de
  prélèvement, horodatage, règles de compte (multisig, plafond), frais
  parrainés, jetons natifs (protocole 5), réserve par objet (protocole 6).
- **Consensus** : BFT de type **Tendermint**, PoA, `n = 3f + 1`, finalité
  immédiate. Ce n'est pas un algorithme maison, et il ne faut jamais le
  présenter comme tel. « VinX Consensus » désigne le **modèle de réseau**.
- **Économie** : supply fixe de 1 Md de VINX. 900 M en escrow (120 tranches
  de 7,5 M, libérées par la gouvernance, testnet uniquement) ; 100 M
  d'émission pour les validateurs (demi-vie 10 ans) ; les frais vont aux
  validateurs.
- **Réseau principal (à venir)** : les 900 M deviennent une réserve gérée
  par le protocole, avec des dépenses votées par les validateurs puis par les
  détenteurs (`docs/adr/0003-reserve-et-gouvernance.md`). Le testnet n'est
  pas modifié (engagement de l'ADR 0001).
- **Gouvernance** : multisig admin, 2 signatures sur 3.

## 3. Arborescence

| Crate | Rôle |
|---|---|
| `vinx-crypto` | Clés (ed25519), adresses `vinx1…`, BLAKE3, Merkle, étiquettes de domaine |
| `vinx-core` | Types : transactions (`TxBody`), actions admin, blocs, votes, paramètres |
| `vinx-state` | Machine d'état : `state.rs` (application des tx et des blocs, invariant, racine), tables (`table.rs`), actifs, réserve, `staking.rs` (protocole 7) |
| `vinx-consensus` | Moteur Tendermint (`engine.rs`), garde anti-double signature, preuves d'équivoque |
| `vinx-node` | Nœud : réseau libp2p, stockage redb (`store.rs`), API axum (`api.rs`, `openapi.json`), pages `dashboard.html`, `explorer.html`, `docs.html` |
| `vinx-wallet` | CLI, portefeuille de bureau (`app.rs`, `app.html`), cockpit admin (`cockpit.rs`, `cockpit.html`) |
| `vinx-mobile` | Portefeuille mobile (WASM + Capacitor, APK par la CI) |
| `vinx-keystore` | Fichiers de clés chiffrés |
| `fuzz/` | Cibles cargo-fuzz (hors espace de travail), lancées chaque nuit |

Documentation : `docs/` (guides de déploiement, d'exploitation, des jetons,
du nœud public, des exercices de panne, `PROTOCOLE-7.md`, ADR dans
`docs/adr/`).

## 4. Commandes

```bash
cargo fmt --all                                         # toujours avant un commit
cargo clippy -q --workspace --all-targets -- -D warnings
cargo test -q --workspace                               # environ 3 minutes
cargo deny check                                        # failles et licences
cd fuzz && cargo +nightly fuzz run --target x86_64-unknown-linux-gnu net_message -- -max_total_time=60

# Réseau local
export VINX_KEY_PASSWORD=pw
vinx-node devnet --dir net
vinx-node run --genesis net/genesis.json --key net/validator-1.json --data-dir net/d1
# admin : vinx-wallet admin propose --out p.json <action> ;
#         admin approve --file p.json --key net/admin-1.json --index 0 (puis 2 / 1) ;
#         admin submit --file p.json
```

La version est dans `Cargo.toml` (`version = "…"`) **et** dans
`crates/vinx-mobile/www/app.js` (`const VERSION`). Les deux sont à changer
ensemble ; vérifier avec `grep '^version' Cargo.toml`.

## 5. Conventions et pièges connus

- **Lints stricts** : pas d'`unwrap`, pas d'arithmétique non vérifiée
  (`checked_*` ou `saturating_*`), pas d'indexation directe dans le code de
  production. Les tests ont leur propre `#![allow(...)]`.
- **Changer une règle = nouvelle version de protocole**, gardée par
  `self.protocol_version >= N`. On n'en retire jamais. On ne touche jamais à
  la supply ni à l'escrow par une mise à jour.
- **Ajouter un champ à l'état** :
  - nouvelle structure `Globals`, l'ancienne passant en `GlobalsVn`, lue en
    repli dans `from_parts` ;
  - lignes de table dans `take_dirty_rows`, `load_row` et
    `mark_all_rows_dirty`, avec une étiquette `Row::TAG` unique (déjà prises :
    1 à 13) ;
  - racine d'état (`state_root`) seulement à partir de la nouvelle version ;
  - invariant (`check_invariant`) si des unités sont bloquées
    (`Row::locked`).
- **Garder les tests compatibles** : `GENESIS_PROTOCOL` vaut toujours 6. Les
  tests du protocole 7 passent par une vraie mise à jour (voir
  `crates/vinx-state/tests/staking.rs`, `net7()`).
- **Shell** : `pkill -x vinx-node` (jamais `pkill -f`, qui tue le shell) ;
  pas de `rm -rf *` ; en cas de disque plein, supprimer `target/debug`.
- **Devnet accéléré** (`min_block_interval = 20`) : garder une **marge** sur
  les hauteurs, par exemple programmer une mise à jour à `H + 18500` et non
  `H + 17300`.
- **Tests multi-nœuds** : ne jamais verrouiller `node::lock(&shared)` deux
  fois dans la même expression, sinon c'est un interblocage.
- **Branche** : travailler sur `claude/wizardly-gauss-2u386c`. Si l'ancienne
  branche distante ne contient que de l'historique déjà fusionné, la
  rattraper avec
  `git merge -s ours origin/claude/wizardly-gauss-2u386c` plutôt qu'un
  force-push (refusé).
- **CI** : formatage, Clippy, tests (Linux, Windows, macOS), cargo-deny,
  APK, fuzzing quand `fuzz/` change. Releases avec attestation de
  provenance ; APK signée avec une clé fixe si les secrets `ANDROID_*`
  existent.

## 6. Déploiement du propriétaire (testnet 2)

- Chaîne `vinx-testnet-2`.
- 4 validateurs sur Proxmox : **Cinnamon** (192.168.1.22), **Amber** (.21),
  **Obsidian** (.23), **Pearl** (.233). Les noms viennent pour l'instant de
  `/etc/vinx/validator-names.json`.
- Nœud public **Copper** (`vinx-public`, 192.168.1.244, mode archive,
  `--node-name Copper`), servi sur https://vinx-ledger.vinxlabs.fr (`/explorer`,
  `/docs`).
- On joint les validateurs **depuis `vinx-public`** (`ssh root@192.168.1.X`).
  Mise à jour d'un nœud :

  ```bash
  wget …/releases/download/vX/vinx-X-linux-x86_64.tar.gz
  tar xzf …
  ./install-validator.sh upgrade
  ```

  Un nœud à la fois, validateurs d'abord, `vinx-public` en dernier.
- Cockpit et clés admin sur son PC Windows : `C:\Users\Administrateur\vinx-cockpit`.
  Il accède aux nœuds par tunnels SSH
  (`ssh -N -L 18541:127.0.0.1:8545 root@IP`).
- Dernière version publiée côté code : 0.28.0. Protocole 6 en cours
  d'activation sur le testnet ; à vérifier dans l'onglet Gouvernance du
  cockpit.

## 7. Chantier en cours : protocole 7

Spécification **validée** : `docs/PROTOCOLE-7.md` (décisions au §9). Une
**seule** version de protocole (7), publiée seulement quand les trois lots
sont terminés et testés ensemble.

**Ne pas fusionner dans `main` avant la fin de P7c** : la branche a déjà
`SUPPORTED_PROTOCOL = 7`, alors que la version est encore incomplète.

### P7a : fait (branche `claude/wizardly-gauss-2u386c`, commit « Protocole 7, lot P7a »)

- `vinx-state/src/staking.rs` :
  - `StakingParams` ;
  - `ValidatorRecord` (identité, site, contact, statut
    `Active | Jailed{since} | Leaving{unlock_at} | Left | Banned`,
    `joined_at`, `grace_until`), TAG 10 ;
  - `NameOwner`, TAG 11 ;
  - `Bond`, TAG 12, compté dans l'invariant ;
  - `PendingAdmin`, TAG 13.
- Transactions `DepositBond { key, amount }` (0x1D) et
  `UpdateValidatorInfo` (0x1E).
- Actions admin `AddNamedValidator` (0x09), `SetValidatorIdentity` (0x0A),
  `CancelPending` (0x0B). `AddValidator` est refusée à partir du
  protocole 7.
- Délai admin (`AdminAction::is_delayed`, `run_pending_admin`), diversité,
  fiches provisoires « validateur-N » à l'activation, avec 30 jours de grâce.
- API : `/validators` (`record`, `registry`) et `/governance` (`staking`,
  `pending_admin`).
- CLI : `vinx-wallet validator bond|info`, nouvelles propositions admin.
- Vérifié : tests `vinx-state/tests/staking.rs`, et essai réel de mise à
  jour 6 → 7 sur devnet.

### P7b : à faire (conception retenue)

- **Format de bloc versionné** :
  - pour `protocol_version >= 7`, le `Block` porte en plus `last_commit`, le
    certificat du bloc précédent avec **toutes** les signatures reçues ;
  - encodage borsh manuel : champ ajouté en fin de bloc seulement si
    `v >= 7`, pour que les anciens blocs restent lisibles ;
  - `last_commit` est engagé dans `tx_root` (son empreinte s'ajoute à la
    liste quand `v >= 7`) ;
  - vérifié dans `apply_block` : hauteur `h - 1`, `block_hash == prev_hash`,
    signatures des validateurs de `h - 1`, quorum.
- **Exécution** : `execute_by` doit recevoir les signataires pour compter la
  présence (proposeur dans `chain.rs` et vérification). Le nœud remplit
  `last_commit` à la construction du bloc, à partir du certificat stocké.
- **Présence et récompenses** :
  - par époque, compteurs signés / attendus par validateur ;
  - éligible si `signés / attendus ≥ min_availability` (9 700) ;
  - pot partagé entre les éligibles : `vote_share` (50 %) au prorata des
    signatures, le reste au prorata des propositions ;
  - rien pour un validateur mis à l'écart pendant l'époque, ni sans caution
    suffisante après la grâce ;
  - sans éligible, le pot est conservé.
- **Mise à l'écart** :
  - absent de `last_commit` pendant `absence_blocks` (1 000) d'affilée :
    `Jailed`, retiré à `H + 2`, jamais si l'ensemble deviendrait vide ;
  - `Unjail` (tx) après `jail_min_blocks`, avec une caution suffisante.
- **Départ** : `LeaveValidators` (tx), puis `WithdrawBond` après
  `bond_unlock_delay`. `RemoveValidator` d'un validateur mis à l'écart :
  seulement passer en `Leaving`.

### P7c : à faire

- `ReportEquivocation { evidence }` : preuve vérifiée par la clé seule (voir
  `vinx-consensus/src/evidence.rs`). Effet :
  - caution confisquée à 100 % et partagée à parts égales entre les autres
    validateurs actifs, le reliquat allant au pot ;
  - statut `Banned`, retrait à `H + 2` ;
  - identifiant de la preuve conservé pour ne sanctionner qu'une fois ;
  - acceptée aussi pendant le délai de restitution.

  Les nœuds soumettent la preuve d'eux-mêmes.
- Interfaces : explorateur (fiches, caution, présence), cockpit (actions en
  attente, annulation, propositions de validateur nommé), portefeuille.
- Fuzzing des nouvelles transactions ; tests multi-nœuds à 4 puis
  7 validateurs ; essai complet de mise à jour 6 → 7 avec pannes.
- Passer `GENESIS_PROTOCOL` à 7 avec fiches et cautions dans la genèse, en
  adaptant les tests.
- Puis : PR unique, release, guide `docs/MISE-A-JOUR-PROTOCOLE-7.md`, et
  ensuite seulement le testnet.

## 8. Après le protocole 7 (feuille de route)

- Politique et module de gouvernance du réseau principal (ADR 0003).
- Préparation du réseau principal : validateurs externes, audit de sécurité
  externe, cadre juridique (MiCA), genèse.
- À la demande d'un premier client : SDK JavaScript, notifications de
  paiement, documentation en anglais.
