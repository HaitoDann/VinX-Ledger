# Décisions d'architecture (ADR) — VinX Ledger

Ce dossier trace toutes les décisions d'architecture de VinX Ledger. Chaque ADR a un **statut** :

| Statut | Signification |
|---|---|
| `Vérifié` ✅ | Implémenté **et** validé sur banc réel ou en production |
| `Implémenté` 🔧 | Code en production, validation banc multi-nœuds en cours |
| `En cours` 🚧 | Branche ouverte, code en écriture |
| `Accepté` 📐 | Design décidé, critères écrits, code pas encore commencé |
| `Proposé` 💡 | Problème posé, options envisagées, pas encore décidé |
| `Remplacé` ❌ | Supersédé par un autre ADR |

**Règle absolue (CONTRIBUTING.md) :** un ADR ne peut passer de Proposé → Accepté que si ses
**critères de validation sont écrits et vérifiables**. Pas de code avant l'état Accepté.

---

## Vue d'ensemble des phases

| Phase | Nom | Objectif | ADRs centraux | État |
|---|---|---|---|---|
| 1 | *(à nommer)* | L1 solide : socle crypto, consensus BFT, émission progressive, finalité | 0051–0063, 0002–0003, 0005–0011, 0015, 0020–0022, 0026–0027, 0031, 0040, 0043, 0045, 0046 | ✅ Terminé |
| **1.5** | **Durcissement & lancement** | **Sécurité du consensus après audit, amorçage, processus de release, critères de lancement** | **0069–0080**, 0064 | 🔄 **En cours** |
| 2 | *(à nommer)* | PoS Algorand-style : comité VRF, pool permissionless, récompenses époque | 0029, 0038, 0028 | 🔄 En cours |
| 3 | *(à nommer)* | Réseau public : mainnet, décentralisation à l'échelle, light client | 0013, 0014, 0016, 0018, 0019 | 🔮 Vision |
| ❄️ | Gelé hors scope (ADR 0064) | Appchains ZK / SP1 / Celestia / ForceExit / Clearinghouse / modules bondés / tokenomics Appchains | 0010, 0023, 0024, 0034, 0039, 0041, 0044, 0047, 0048, 0049, 0050 | ❌ Abandonné |

> **Périmètre produit :** [ADR 0064](./0064-vinx-rail-paiement-uniquement.md) fixe VinX comme
> **rail de paiement L1 minimaliste sans VM**. Les phases 3 et 4 sont gelées : aucune
> implémentation avant qu'un besoin soit démontré post-testnet et réexaminé dans un nouvel ADR.
>
> **Porte de sortie :** [ADR 0080](./0080-criteres-lancement-testnet-mainnet.md) énumère les
> conditions vérifiables du testnet public puis du mainnet. Aucun critère n'est aujourd'hui
> entièrement satisfait.

---

## Phase 1 — Socle L1 ✅ Terminé

### Cryptographie & identité

| ADR | Titre | Statut |
|---|---|---|
| [0051](./0051-primitives-cryptographiques-ed25519-bech32.md) | Primitives crypto : Ed25519, Bech32, Merkle (hash → BLAKE3, ADR 0069) | ✅ Vérifié |
| [0052](./0052-keystore-chiffrement-wallet.md) | Keystore — chiffrement du fichier portefeuille | ✅ Vérifié |
| [0046](./0046-bls-aggregate-cosignatures.md) | BLS12-381 — agrégation des co-signatures | ✅ Vérifié |

### Transactions & anti-replay

| ADR | Titre | Statut |
|---|---|---|
| [0053](./0053-anti-replay-nonce-chainid-expiry.md) | Anti-replay : nonce, chain_id, expiry_height | ✅ Vérifié |
| [0054](./0054-types-transactions-fondamentaux.md) | Types de transactions fondamentaux (0x01–0x0A) | ✅ Vérifié |
| [0008](./0008-chain-id-defaut-sur.md) | chain_id par défaut sûr (pas de défaut silencieux) | ✅ Vérifié |
| [0020](./0020-serialisation-canonique.md) | Sérialisation canonique consensus-critique (Borsh/bincode + vecteurs dorés) | ✅ Vérifié |
| [0035](./0035-bornes-ressources-transaction.md) | Bornes de ressources par transaction (payload, poids bloc) | 💡 Proposé |

### Économie de base & frais

| ADR | Titre | Statut |
|---|---|---|
| [0055](./0055-modele-frais-forfaitaire.md) | Modèle de frais forfaitaire (flat fee) | ✅ Vérifié |
| [0009](./0009-frais-stake-unstake.md) | Frais des transactions stake/unstake (exemption + plafond anti-spam) | ✅ Vérifié |
| [0056](./0056-bond-unbonding-mecanism-staking.md) | Bond & queue de déliaison (mécanisme de staking) | ✅ Vérifié |

### État & persistance

| ADR | Titre | Statut |
|---|---|---|
| [0062](./0062-world-state-structure.md) | WorldState — structure de l'état du monde | ✅ Vérifié |
| [0061](./0061-stockage-persistant-storage-version.md) | Stockage persistant & STORAGE_VERSION (migrations) | ✅ Vérifié |
| [0026](./0026-depot-existentiel.md) | Dépôt existentiel — anti-bloat de l'état | ✅ Vérifié |

### Consensus PoA Threshold (implémentation initiale)

| ADR | Titre | Statut |
|---|---|---|
| [0063](./0063-consensus-poa-threshold-initial.md) | Consensus PoA Threshold — round-robin + quorum BFT | 🔧 Implémenté (remplacé par ADR 0029) |
| [0002](./0002-finalite-quorum-prefix-closed.md) | Finalité au quorum prefix-closed | ✅ Vérifié (tranche 1) |
| [0003](./0003-slashing-automatique-equivocation.md) | Slashing automatique de l'équivocation | ✅ Vérifié |
| [0027](./0027-fiabilite-jailing-validateurs.md) | Fiabilité & jailing des validateurs | ✅ Vérifié (t1+t2a+t2b) |
| [0031](./0031-regle-fork-choice.md) | Règle de fork-choice (canonical_head) | ✅ Vérifié (t1+t2a+t2b) |
| [0043](./0043-parametres-cadence-consensus.md) | Paramètres de cadence : 12 s, 3 000 tx/bloc | ✅ Vérifié |
| [0045](./0045-cadence-fixe-abolition-heartbeat.md) | Cadence fixe 12 s — abolition du heartbeat | ✅ Vérifié |
| [0015](./0015-execution-parallele.md) | Exécution parallèle (vérification signatures multi-cœurs) | ✅ Vérifié (tranche 1) |
| [0005](./0005-temps-reseau-robuste.md) | Temps réseau robuste (Median Time Past) | ✅ Vérifié |

### Émission & tokenomics fondamentaux

| ADR | Titre | Statut |
|---|---|---|
| [0040](./0040-emission-progressive-sans-fonderie.md) | Émission progressive sans La Fonderie (T_half=20 ans) | ✅ Vérifié |
| [0021](./0021-immutabilite-emission.md) | Immutabilité de la courbe d'émission | ✅ Accepté (révisé par 0040) |
| [0004](./0004-invariant-supply-executable.md) | Invariant de supply exécutable | ❌ Remplacé par ADR 0040 |

### Réseau P2P & API

| ADR | Titre | Statut |
|---|---|---|
| [0058](./0058-couche-p2p-gossipsub-base.md) | Couche P2P de base (libp2p + gossipsub) | ✅ Vérifié |
| [0022](./0022-durcissement-p2p.md) | Durcissement P2P / anti-DoS (guard, rate-limit, zstd) | ✅ Vérifié (tranche 1) |
| [0059](./0059-api-rpc-rest.md) | API RPC REST (routes, admin fail-closed, faucet) | ✅ Vérifié |
| [0060](./0060-explorateur-blocs-embarque.md) | Explorateur de blocs embarqué (UI HTML statique) | ✅ Vérifié |
| [0057](./0057-mempool.md) | Mempool — file d'attente de transactions | ✅ Vérifié |

### Gouvernance

| ADR | Titre | Statut |
|---|---|---|
| [0007](./0007-unification-gouvernance.md) | Unification des chemins de gouvernance (AdminAction unique) | ✅ Vérifié |
| [0011](./0011-decentralisation-gouvernance.md) | Décentralisation de la gouvernance (multisig K-of-M) | ✅ Vérifié (tranche 1) |
| [0006](./0006-preavis-upgrade-temps-reel.md) | Préavis d'upgrade en temps réel (timestamp, pas hauteur) | ✅ Vérifié |

### Modules & ancrage (fondation)

| ADR | Titre | Statut |
|---|---|---|
| [0001](./0001-l1-monnaie-pure-modules-ancrage-bonde.md) | L1 monnaie pure + surcouches par ancrage bondé (doctrine) | 📐 Accepté (socle) |
| [0010](./0010-primitive-ancrage-modules.md) | Primitive d'ancrage & registre de modules (AnchorState 0x09) | ✅ Vérifié (tranche 1) |

### Wallet & outils

| ADR | Titre | Statut |
|---|---|---|
| [0033](./0033-genese-bootstrap-fair-launch.md) | Genèse & bootstrap de fair-launch | 📐 Partiellement supersédé |

---

## Phase 2 — PoS Algorand-style 🔄 En cours

### Consensus cible

| ADR | Titre | Statut | Priorité |
|---|---|---|---|
| [0029](./0029-agregation-signatures-comite-dynamique.md) | Comité VRF Algorand-style : ECVRF RFC 9381 + BLS12-381 | 🔧 Phase 1 + 2a implémentées (comité `k<N` & finalité par comité = 2b/2c) | 🔴 Haute |
| [0038](./0038-open-poa-admission.md) | Open PoS — admission permissionless au pool de validateurs | 🔧 Implémenté (sélection VRF en attente de 0029) | 🔴 Haute |
| [0036](./0036-bornes-churn-validateurs.md) | Bornes de churn du set de validateurs | 💡 Proposé | 🟠 Moyenne |

### Économie validateurs

| ADR | Titre | Statut | Priorité |
|---|---|---|---|
| [0028](./0028-partage-emission-quorum.md) | Partage de l'émission par époque (PROPOSER_SHARE_BPS) | 📐 Accepté | 🟠 Moyenne (en parallèle de 0029) |

### Sécurité BFT

| ADR | Titre | Statut | Priorité |
|---|---|---|---|
| [0030](./0030-accountability-cosignatures-conflictuelles.md) | Accountability des co-signatures conflictuelles | 💡 Proposé | 🔴 Haute |
| [0032](./0032-garde-fous-gouvernance.md) | Garde-fous de gouvernance | 💡 Brouillon | 🟠 Moyenne |

---

## Phase 1.5 — Durcissement & lancement 🔄 En cours

Décisions issues de l'**audit contradictoire de septembre 2026** et des manques révélés en
préparant un lancement public. Le dossier d'audit complet — matrice des findings, preuves
d'exploitation, correctifs, prompts de contre-audit — vit dans
[`audit/post-fix/`](../../audit/post-fix/).

### Sécurité du consensus (correctifs d'audit, implémentés)

| ADR | Titre | Statut | Finding |
|---|---|---|---|
| [0069](./0069-blake3-remplace-sha256.md) | BLAKE3 remplace SHA-256 ⚠️ *hard fork*, avant genesis | 🔧 Implémenté | — |
| [0070](./0070-authentification-proposeur-registre-bls.md) | Authentification du proposeur & liaison au registre BLS | 🔧 Implémenté | VINX-01/02/05 |
| [0071](./0071-verrou-vote-persistant.md) | Verrou de vote persistant — une signature par hauteur | 🔧 Implémenté | VX-RED-003/007 |
| [0072](./0072-state-root-engage-consensus.md) | `state_root` engage l'état de consensus ⚠️ *hard fork* | 🔧 Implémenté | VINX-04 |
| [0073](./0073-injectivite-serialisation-signee.md) | Injectivité de la sérialisation signée ⚠️ *hard fork* | 🔧 Implémenté | VINX-12 |

### Amorçage & exploitation

| ADR | Titre | Statut | Priorité |
|---|---|---|---|
| [0075](./0075-genese-enrolement-validateurs.md) | Cérémonie de genèse & enrôlement des validateurs | 🔧 Partiel | 🔴 Haute |
| [0076](./0076-gestion-operationnelle-cles-validateur.md) | Gestion opérationnelle des clés de validateur | 🔧 Partiel | 🔴 Haute |
| [0074](./0074-synchronisation-etat-subjectivite-faible.md) | Synchronisation d'état & subjectivité faible | 🔧 Partiel | 🔴 Haute (mainnet) |

### Produit & processus

| ADR | Titre | Statut | Priorité |
|---|---|---|---|
| [0077](./0077-finalite-paiement-garanties-confirmation.md) | Finalité de paiement : garantie & règle de confirmation | 📐 Accepté | 🔴 Haute |
| [0078](./0078-divulgation-vulnerabilites-reponse-incident.md) | Divulgation des vulnérabilités & réponse à incident | 📐 Accepté | 🔴 Haute |
| [0079](./0079-release-versioning-upgrade-reseau.md) | Release, versioning & upgrade réseau | 📐 Accepté | 🟠 Moyenne |
| [0080](./0080-criteres-lancement-testnet-mainnet.md) | **Critères de lancement : testnet → mainnet** | 📐 Accepté | 🔴 Porte de sortie |
| [0081](./0081-decisions-irreversibles-pre-genesis.md) | **Décisions irréversibles avant la genèse** (supply, décimales, frais, Bech32m, type de clé, gouvernance, borsh) | ✅ Implémenté | 🔴 Haute |
| [0082](./0082-consensus-bft-tendermint.md) | **Consensus BFT par étapes (Tendermint)** — rotation pondérée, votes plafonnés à 10 %, ≤ 100 validateurs, temps de bloc de genèse | ✅ Implémenté | 🔴 Haute |
| [0083](./0083-etat-leger.md) | **État léger** — arbre de Merkle creux, élagage 30 j, snapshots, `--archive`, reçus de paiement | ✅ Implémenté | 🔴 Haute |

> **Le banc adversarial multi-nœuds est désormais en place** (ADR 0080 §2.2) : 6 scénarios
> avec un `Storage` réel (redb) par nœud, validés par mutation testing (non-équivocation, rejet
> des intrus, entrées malformées inertes, pas de finalité conflictuelle sous partition). Reste
> ouvert : la cérémonie de genèse multi-validateurs (0075 §1) et le recoupement multi-pairs des
> checkpoints (0074).

---

## ❄️ Appchains ZK & économie de modules — Gelé hors scope (ADR 0064)

> Le recentrage v6.0 a **abandonné** tout l'écosystème Appchains / ZK / modules bondés. Ces ADR
> sont **conservés pour mémoire** — ni implémentés, ni planifiés. VinX est un rail de paiement,
> pas un settlement layer. Aucun type de transaction ZK / ForceExit / CrossMsg n'existe dans le code.

| ADR | Titre | Statut |
|---|---|---|
| [0050](./0050-sp1-proof-verification-l1.md) | Vérification des preuves SP1 Groth16 sur le L1 | ❄️ Gelé (0064) |
| [0034](./0034-disponibilite-donnees-verification-ancre.md) | Disponibilité des données — Celestia DA | ❄️ Gelé (0064) |
| [0048](./0048-force-exit-escape-hatch.md) | ForceExit / Escape Hatch | ❄️ Gelé (0064) |
| [0049](./0049-clearinghouse-cross-appchain.md) | Clearinghouse cross-Appchain | ❄️ Gelé (0064) |
| [0023](./0023-adjudication-slashing-module.md) | Adjudication du slashing de module | ❄️ Gelé (0064) |
| [0024](./0024-infrastructure-subnets-escrow-recompense.md) | Infrastructure de subnets : escrow bondé + racine de récompense | ❄️ Gelé (0064) |
| [0010](./0010-primitive-ancrage-modules.md) | Primitive d'ancrage de modules (`AnchorState 0x09`) | ❄️ Gelé (0064) — primitive résiduelle conservée |
| [0039](./0039-remuneration-operateurs-modules.md) | Rémunération des opérateurs de modules | ❄️ Gelé (0064) |
| [0041](./0041-repartition-emission-usage-melt.md) | Répartition de l'émission entre Appchains par usage (melt) | ❄️ Gelé (0064) |
| [0044](./0044-garde-fous-equite-amorcage-emission.md) | Garde-fous d'équité et d'amorçage de l'émission | ❄️ Gelé (0064) |
| [0047](./0047-emission-elastique-reservoir.md) | Émission élastique à réservoir | ❄️ Gelé (0064) |

---

## Phase 3 — Réseau public 🔮 Vision mainnet

| ADR | Titre | Statut | Priorité |
|---|---|---|---|
| [0014](./0014-standard-light-client.md) | Standard light-client (balance proof + checkpoints) — **reste pertinent pour le rail de paiement** | 💡 Proposé | 🟠 Moyenne |
| [0042](./0042-epoque-reglement-emission.md) | Époque de règlement de l'émission (tokenomics de base, non lié aux Appchains) | 💡 Proposé | 🟠 Moyenne |
| [0013](./0013-cycle-vie-etat.md) | Cycle de vie de l'état (loyer & expiration des comptes dormants) | 💡 Proposé | 🟢 Futur |
| [0016](./0016-posture-post-quantique.md) | Posture post-quantique (chemin de migration ML-DSA) | 💡 Proposé | 🟢 Futur |
| [0018](./0018-observabilite-slo.md) | Observabilité & SLO (métriques, alertes, runbooks) | 💡 Proposé | 🟢 Futur |
| [0019](./0019-tls-natif-rustls.md) | TLS natif rustls (HTTPS sans reverse-proxy) | 💡 Proposé | 🟢 Futur |

---

## Proposés — toutes phases

### Sécurité opérationnelle

| ADR | Titre | Statut | Priorité |
|---|---|---|---|
| [0012](./0012-gestion-cles-validateur.md) | Gestion des clés validateur (remote signer, clé P2P distincte) | 💡 Proposé | 🟢 Futur |
| [0017](./0017-arret-urgence-reprise.md) | Arrêt d'urgence & reprise (EmergencyHalt par gouvernance) | 💡 Proposé | 🟢 Futur |

### Réseau P2P

| ADR | Titre | Statut | Priorité |
|---|---|---|---|
| [0037](./0037-propagation-compacte-blocs.md) | Propagation compacte des blocs (CompactBlock) | 💡 Proposé | 🟢 Futur |

---

## Index complet par numéro

| # | Titre court | Statut | Phase |
|---|---|---|---|
| [0001](./0001-l1-monnaie-pure-modules-ancrage-bonde.md) | L1 monnaie pure + ancrage bondé | 📐 Accepté | 1 |
| [0002](./0002-finalite-quorum-prefix-closed.md) | Finalité au quorum prefix-closed | ✅ Vérifié | 1 |
| [0003](./0003-slashing-automatique-equivocation.md) | Slashing automatique équivocation | ✅ Vérifié | 1 |
| [0004](./0004-invariant-supply-executable.md) | Invariant de supply exécutable | ❌ Remplacé (→ 0040) | 1 |
| [0005](./0005-temps-reseau-robuste.md) | Temps réseau robuste (MTP) | ✅ Vérifié | 1 |
| [0006](./0006-preavis-upgrade-temps-reel.md) | Préavis d'upgrade en temps réel | ✅ Vérifié | 1 |
| [0007](./0007-unification-gouvernance.md) | Unification gouvernance | ✅ Vérifié | 1 |
| [0008](./0008-chain-id-defaut-sur.md) | chain_id par défaut sûr | ✅ Vérifié | 1 |
| [0009](./0009-frais-stake-unstake.md) | Frais stake/unstake | ✅ Vérifié | 1 |
| [0010](./0010-primitive-ancrage-modules.md) | Primitive ancrage & registre modules | ✅ Vérifié | 1 |
| [0011](./0011-decentralisation-gouvernance.md) | Décentralisation gouvernance K-of-M | ✅ Vérifié | 1 |
| [0012](./0012-gestion-cles-validateur.md) | Gestion clés validateur | 💡 Proposé | 5 |
| [0013](./0013-cycle-vie-etat.md) | Cycle de vie de l'état (loyer) | 💡 Proposé | 5 |
| [0014](./0014-standard-light-client.md) | Standard light-client | 💡 Proposé | 3 |
| [0015](./0015-execution-parallele.md) | Exécution parallèle (vérif multi-cœurs) | ✅ Vérifié | 1 |
| [0016](./0016-posture-post-quantique.md) | Posture post-quantique | 💡 Proposé | 5 |
| [0017](./0017-arret-urgence-reprise.md) | Arrêt d'urgence & reprise | 💡 Proposé | — |
| [0018](./0018-observabilite-slo.md) | Observabilité & SLO | 💡 Proposé | 5 |
| [0019](./0019-tls-natif-rustls.md) | TLS natif rustls | 💡 Proposé | 5 |
| [0020](./0020-serialisation-canonique.md) | Sérialisation canonique | ✅ Vérifié | 1 |
| [0021](./0021-immutabilite-emission.md) | Immutabilité courbe émission | ✅ Accepté | 1 |
| [0022](./0022-durcissement-p2p.md) | Durcissement P2P anti-DoS | ✅ Vérifié | 1 |
| [0023](./0023-adjudication-slashing-module.md) | Adjudication slashing module | ❄️ Gelé (0064) | — |
| [0024](./0024-infrastructure-subnets-escrow-recompense.md) | Infrastructure subnets escrow | ❄️ Gelé (0064) | — |
| [0026](./0026-depot-existentiel.md) | Dépôt existentiel anti-bloat | ✅ Vérifié | 1 |
| [0027](./0027-fiabilite-jailing-validateurs.md) | Fiabilité & jailing validateurs | ✅ Vérifié | 1 |
| [0028](./0028-partage-emission-quorum.md) | Partage émission par époque | 📐 Accepté | 2 |
| [0029](./0029-agregation-signatures-comite-dynamique.md) | Comité VRF Algorand-style + BLS | 🔧 Phase 1 + 2a implémentées | 2 |
| [0030](./0030-accountability-cosignatures-conflictuelles.md) | Accountability co-signatures conflictuelles | 💡 Proposé | 2 |
| [0031](./0031-regle-fork-choice.md) | Règle de fork-choice (canonical_head) | ✅ Vérifié | 1 |
| [0032](./0032-garde-fous-gouvernance.md) | Garde-fous de gouvernance | 💡 Brouillon | 2 |
| [0033](./0033-genese-bootstrap-fair-launch.md) | Genèse & bootstrap fair-launch | 📐 Partiellement supersédé | 1 |
| [0034](./0034-disponibilite-donnees-verification-ancre.md) | Disponibilité données Celestia DA | ❄️ Gelé (0064) | — |
| [0035](./0035-bornes-ressources-transaction.md) | Bornes ressources par transaction | 💡 Proposé | 1 |
| [0036](./0036-bornes-churn-validateurs.md) | Bornes churn validateurs | 💡 Proposé | 2 |
| [0037](./0037-propagation-compacte-blocs.md) | Propagation compacte blocs | 💡 Proposé | — |
| [0038](./0038-open-poa-admission.md) | Open PoS admission permissionless | 🔧 Implémenté (VRF en attente 0029) | 2 |
| [0039](./0039-remuneration-operateurs-modules.md) | Rémunération opérateurs modules | ❄️ Gelé (0064) | — |
| [0040](./0040-emission-progressive-sans-fonderie.md) | Émission progressive sans La Fonderie | ✅ Vérifié | 1 |
| [0041](./0041-repartition-emission-usage-melt.md) | Répartition émission Appchains par melt | ❄️ Gelé (0064) | — |
| [0042](./0042-epoque-reglement-emission.md) | Époque règlement émission | 💡 Proposé | 2 |
| [0043](./0043-parametres-cadence-consensus.md) | Cadence 12 s, 3 000 tx/bloc | ✅ Vérifié | 1 |
| [0044](./0044-garde-fous-equite-amorcage-emission.md) | Garde-fous équité amorçage émission | ❄️ Gelé (0064) | — |
| [0045](./0045-cadence-fixe-abolition-heartbeat.md) | Cadence fixe — abolition heartbeat | ✅ Vérifié | 1 |
| [0046](./0046-bls-aggregate-cosignatures.md) | BLS12-381 agrégation co-signatures | ✅ Vérifié | 1 |
| [0047](./0047-emission-elastique-reservoir.md) | Émission élastique à réservoir | ❄️ Gelé (0064) | — |
| [0048](./0048-force-exit-escape-hatch.md) | ForceExit / Escape Hatch | ❄️ Gelé (0064) | — |
| [0049](./0049-clearinghouse-cross-appchain.md) | Clearinghouse cross-Appchain | ❄️ Gelé (0064) | — |
| [0050](./0050-sp1-proof-verification-l1.md) | Vérification preuves SP1 Groth16 | ❄️ Gelé (0064) | — |
| [0051](./0051-primitives-cryptographiques-ed25519-bech32.md) | Primitives crypto (Ed25519, Bech32, Merkle ; hash → BLAKE3, ADR 0069) | ✅ Vérifié | 1 |
| [0052](./0052-keystore-chiffrement-wallet.md) | Keystore — chiffrement wallet (Argon2id + AES-256-GCM) | ✅ Vérifié | 1 |
| [0053](./0053-anti-replay-nonce-chainid-expiry.md) | Anti-replay : nonce, chain_id, expiry_height | ✅ Vérifié | 1 |
| [0054](./0054-types-transactions-fondamentaux.md) | Types de transactions fondamentaux (0x01–0x0C) | ✅ Vérifié | 1 |
| [0055](./0055-modele-frais-forfaitaire.md) | Modèle de frais forfaitaire (flat fee) | ✅ Vérifié | 1 |
| [0056](./0056-bond-unbonding-mecanism-staking.md) | Bond & queue de déliaison (staking) | ✅ Vérifié | 1 |
| [0057](./0057-mempool.md) | Mempool — file d'attente de transactions | ✅ Vérifié | 1 |
| [0058](./0058-couche-p2p-gossipsub-base.md) | Couche P2P de base (libp2p + gossipsub) | ✅ Vérifié | 1 |
| [0059](./0059-api-rpc-rest.md) | API RPC REST (routes, admin, faucet) | ✅ Vérifié | 1 |
| [0060](./0060-explorateur-blocs-embarque.md) | Explorateur de blocs embarqué (UI HTML) | ✅ Vérifié | 1 |
| [0061](./0061-stockage-persistant-storage-version.md) | Stockage persistant & STORAGE_VERSION | ✅ Vérifié | 1 |
| [0062](./0062-world-state-structure.md) | WorldState — structure de l'état du monde | ✅ Vérifié | 1 |
| [0063](./0063-consensus-poa-threshold-initial.md) | Consensus PoA Threshold (implémentation initiale) | 🔧 Implémenté | 1 |
| [0064](./0064-vinx-rail-paiement-uniquement.md) | VinX = rail de paiement uniquement (périmètre) | 📐 Décidé | 1.5 |
| [0065](./0065-allocateur-mimalloc.md) | Allocateur mimalloc | 📐 Décidé — non implémenté | 1 |
| [0066](./0066-hash-transaction-memoize.md) | Hash de transaction mémoïsé | 📐 Décidé — non implémenté | 1 |
| [0067](./0067-cache-signatures-mempool-bloc.md) | Cache de signatures mempool → bloc | 📐 Décidé — non implémenté | 1 |
| [0068](./0068-batch-verify-ed25519.md) | Vérification Ed25519 par lot | 📐 Décidé — non implémenté | 1 |
| [0069](./0069-blake3-remplace-sha256.md) | BLAKE3 en remplacement de SHA-256 ⚠️ | 🔧 Implémenté | 1.5 |
| [0070](./0070-authentification-proposeur-registre-bls.md) | Authentification du proposeur & registre BLS | 🔧 Implémenté | 1.5 |
| [0071](./0071-verrou-vote-persistant.md) | Verrou de vote persistant (1 vote / hauteur) | 🔧 Implémenté | 1.5 |
| [0072](./0072-state-root-engage-consensus.md) | `state_root` engage l'état de consensus ⚠️ | 🔧 Implémenté | 1.5 |
| [0073](./0073-injectivite-serialisation-signee.md) | Injectivité de la sérialisation signée ⚠️ | 🔧 Implémenté | 1.5 |
| [0074](./0074-synchronisation-etat-subjectivite-faible.md) | Sync d'état & subjectivité faible | 🔧 Partiel | 1.5 |
| [0075](./0075-genese-enrolement-validateurs.md) | Genèse & enrôlement des validateurs | 🔧 Partiel | 1.5 |
| [0076](./0076-gestion-operationnelle-cles-validateur.md) | Gestion opérationnelle des clés validateur | 🔧 Partiel | 1.5 |
| [0077](./0077-finalite-paiement-garanties-confirmation.md) | Finalité de paiement & règle de confirmation | 📐 Accepté | 1.5 |
| [0078](./0078-divulgation-vulnerabilites-reponse-incident.md) | Divulgation & réponse à incident | 📐 Accepté | 1.5 |
| [0079](./0079-release-versioning-upgrade-reseau.md) | Release, versioning & upgrade réseau | 📐 Accepté | 1.5 |
| [0080](./0080-criteres-lancement-testnet-mainnet.md) | Critères de lancement testnet → mainnet | 📐 Accepté | 1.5 |
| [0081](./0081-decisions-irreversibles-pre-genesis.md) | Décisions irréversibles avant la genèse | ✅ Implémenté | 1.5 |
| [0082](./0082-consensus-bft-tendermint.md) | Consensus BFT par étapes (Tendermint) | ✅ Implémenté | 1.5 |
| [0083](./0083-etat-leger.md) | État léger (SMT, élagage, snapshots, reçus) | ✅ Implémenté | 1.5 |

> ⚠️ = change les règles de consensus. Appliqué **sans hauteur d'activation**, ce qui n'est
> acceptable qu'en pré-lancement (`0.1.0-alpha.1`, aucun réseau public). Après le lancement,
> voir ADR 0079 §2.3.
>
> *Le numéro 0025 n'a jamais été attribué.*
>
> **ADR 0069 est implémenté** (septembre 2026) : le protocole hache en BLAKE3, avant toute
> genèse publique, comme l'ADR l'exigeait. **ADR 0065–0068 restent décidés mais non
> implémentés** — ce sont des optimisations sans impact consensus, donc non bloquantes pour
> le lancement.

---

## Comment contribuer

1. Copier le gabarit de l'ADR 0001 (Contexte / Options / Décision / Conséquences).
2. Numéroter en séquence (prochain disponible : **0084**).
3. Écrire les **critères de validation vérifiables** avant de passer Accepté.
4. Suivre le cycle : `Proposé → Accepté → En cours → Implémenté → Vérifié`.
5. Mettre à jour ce README et `PROTOCOL_SPEC.md` quand l'ADR passe Implémenté.
6. Classer le changement selon ADR 0079 §2.2 (*consensus-breaking* / compatible réseau /
   local au nœud) **avant** la revue. En cas de doute : consensus-breaking.
7. Un ADR proposant une VM, des modules ou du ZK doit référencer ADR 0064 et démontrer un
   besoin avéré post-testnet.

Voir [CONTRIBUTING.md](../../CONTRIBUTING.md) pour le processus complet.
