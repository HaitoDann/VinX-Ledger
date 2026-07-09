# VinX Ledger — Livre Blanc

**Version :** 3.1
**Date :** Juillet 2026
**Éditeur :** VinX Labs

---

## 1. Vision & Philosophie

VinX Ledger est une **monnaie numérique artisanale**, conçue comme une infrastructure de paiement du quotidien, **fiable et précise au centime près**.

Elle refuse la spéculation et la complexité des smart contracts pour se concentrer sur une promesse simple : un **cash numérique honnête, rapide et souverain**.

Construit en solo, sans investisseurs, sans pre-mine dilué, sans promesses spéculatives, VinX Ledger est une infrastructure financière qui assume son rythme et ses choix.

Cohérent avec cette philosophie, VinX est développé sur un **protocole Rust entièrement maîtrisé**, sans framework tiers imposant sa vision. Chaque ligne de code correspond exactement à ce que VinX veut être — rien de plus.

---

## 2. Architecture Technique

- **Langage** : Rust, implémentation propriétaire de bout en bout
- **Vitesse** : Cadence de bloc **adaptative à la demande** — *repos* → aucun bloc ; *activité normale* → jusqu'à ~5 s (block time), les transactions s'agrègent ; *montée en charge* → l'écart se resserre à mesure que le mempool se remplit ; *saturation* → blocs **dos à dos**. Finalité déterministe immédiate via le consensus PoA Threshold
- **Capacité** : jusqu'à **10 000 transactions par bloc** (réglable), mempool de **100 000** transactions
- **Performance** : plusieurs milliers de TPS en configuration optimisée (dépend du matériel et des réglages ; l'exécution est séquentielle — une exécution parallèle serait requise au-delà)
- **Précision** : 18 décimales internes, 2 décimales affichées à l'utilisateur
- **Adresses** : Format Bech32 avec préfixe `vinx1`
- **Cryptographie** : Ed25519 (signatures), SHA-256 (hachage), Bech32 (adresses)

### Choix du protocole custom

VinX Ledger est implémenté sans framework blockchain tiers. Ce choix garantit :

- **Auditabilité maximale** — surface de code réduite, aucune dépendance opaque
- **Maîtrise totale du protocole** — chaque règle est écrite explicitement, rien n'est hérité par défaut
- **Alignement avec la vision** — une monnaie artisanale mérite une implémentation artisanale
- **Stabilité à long terme** — aucune dépendance upstream susceptible de casser l'API

Les briques P2P (libp2p Rust), consensus PoA Threshold et mises à jour forkless sont développées nativement dans le projet.

---

## 3. Tokenomics & La Fonderie

La supply totale est fixée à **100 000 000 000 VinX** (100 milliards), **immuable et sans burn**. Les jetons ne sont jamais créés ni détruits — ils sont **forgés** au genesis puis **cyclent** perpétuellement entre la circulation et une réserve unique, **La Fonderie**.

> Vocabulaire VinX : on ne *mint* pas des jetons, **on les forge**. On ne *brûle* pas les frais, **on les fond**. C'est le même métal qui circule, fond et se reforge à l'infini.

### 3.1 Le principe melt / forge

- **Melt (fondre)** : 100 % des frais de transaction retournent dans **La Fonderie** et quittent la circulation. Ce n'est **pas** un burn — le métal est conservé.
- **Forge (forger)** : les récompenses de staking sont forgées depuis La Fonderie vers la circulation. Chaque distribution forge une **fraction** de la Fonderie (0,1 %).
- **Cycle infini** : comme la forge ne prend qu'une fraction et que les frais refont fondre du métal en continu, **La Fonderie ne se vide jamais**.

### 3.2 L'invariant fondateur

À **chaque bloc**, sans exception :

```
circulation + Fonderie = 100 000 000 000 VinX   (constant, pour toujours)
```

Rien n'est créé, rien n'est détruit. La valeur ne fait que changer de forme.

### 3.3 Répartition à la genèse

- **1 000 000 000 VinX (1 %)** — forgés au **fondateur** pour amorcer la circulation et distribuer aux premiers utilisateurs.
- **99 000 000 000 VinX (99 %)** — scellés dans **La Fonderie**, forgés progressivement dans l'économie via les récompenses de staking.

---

## 4. Distribution initiale

Le milliard de VinX en circulation à la genèse est **distribué manuellement par le fondateur**, sans émission programmatique automatique, à son rythme.

L'usage vise d'abord un **cercle restreint** (proches, premiers contributeurs, testeurs) pour amorcer une **micro-économie réelle** — transactions, staking, échanges P2P — avant tout élargissement.

---

## 5. Cycle Économique & Frais

Chaque transaction réseau applique des frais de **0,05 %** avec un **plancher minimum de 0,0001 VinX**.

### Melt intégral

- **100 % des frais fondent dans La Fonderie.** Aucun frais direct au validateur ni à une trésorerie séparée.
- Les frais fondus alimentent la réserve d'où sont forgées les récompenses de staking.

```
[Fondateur 1 Md] ──► [Premiers utilisateurs] ──► [Circulation P2P]
                                                        │
   ┌─────────────────────────────────────────────────────┘
   ▼
[Frais 0,05%] ──(melt)──► [La Fonderie] ──(forge)──► [Récompenses staking] ──► circulation ──► …
```

Le cycle est **fermé et perpétuel** : c'est le même métal qui circule, fond et se reforge.

### Staking

- Stake minimum : **1 VinX**
- Pas de période de lock sur le capital — déstaking instantané
- Pas de slashing sur le stake
- **Warm-up de 100 blocs** : un stake ne devient éligible aux récompenses qu'après avoir traversé une époque complète. Cela ferme l'exploit du *just-in-time staking* (staker juste avant une distribution, encaisser, déstaker juste après). Un top-up décale l'ancienneté (`stake_since`) proportionnellement au capital, de sorte qu'un gros dépôt tardif n'hérite pas de l'ancienneté d'un petit stake ancien.
- Récompenses **forgées depuis La Fonderie** toutes les **100 blocs** (~17 minutes), proportionnellement au **stake éligible**

---

## 6. Infrastructure : Validateurs & Full Nodes

### 6.1 Validateurs Core (PoA Threshold)

Liste restreinte de nœuds sélectionnés, opérés par VinX Labs et des partenaires de confiance, **légalement identifiés et responsables**.

**Rôle** : proposer et co-signer les blocs, maintenir le consensus, garantir la disponibilité du réseau.

**Mécanisme** : Proof of Authority Threshold — à chaque bloc, le validateur désigné (rotation déterministe) propose un bloc. Ce bloc est finalisé lorsque **plus de 66% des validateurs actifs** (≥ 14 sur 21 à maturité) l'ont co-signé. La finalité est déterministe et immédiate : un bloc signé ne peut jamais être réorganisé.

**Tolérance aux pannes** : le réseau reste opérationnel tant que 66% des validateurs sont en ligne. Jusqu'à 33% de validateurs hors-ligne ou défaillants sont tolérés sans interruption du service.

**Évolution** : 1 (local) → 3 (redondance), extensible ensuite. Les validateurs ne perçoivent **aucun frais direct** (les frais fondent à 100 % dans La Fonderie) ; comme tout détenteur, ils peuvent staker pour recevoir des récompenses forgées.

### 6.2 Full Nodes Communautaires

**N'importe qui peut faire tourner un nœud complet** sans permission et sans rémunération directe en Phase 1.

**Motivation triple** :
- **Conviction** : souveraineté financière et vérification indépendante
- **Utilité personnelle** : disposer d'un point d'accès RPC privé pour son propre usage
- **Auditabilité** : garantir collectivement que VinX Labs ne triche pas

Les full nodes sont **la couche de redondance** du réseau. Si VinX Labs disparaissait, les full nodes conservent l'intégralité de la blockchain et permettent au réseau de continuer d'exister.

Une éventuelle rémunération des full nodes pourra être étudiée dans les phases futures si l'évolution du réseau le justifie, mais n'est pas engagée en Phase 1.

---

## 7. Gouvernance & Évolution

La gouvernance est assurée par une **clé admin unique** (le fondateur), **rotatable à chaud** via `AdminAction::RotateAdmin` sans redémarrage du nœud. Ses seules prérogatives on-chain : gérer le set de validateurs, ajuster le plancher de frais, et planifier les mises à jour de protocole.

Il n'y a **pas de gel de compte** : la propriété des jetons est inconditionnelle.

> Une répartition du contrôle admin (signature à seuil / multi-parties) pourra être introduite plus tard si le réseau grandit — ce n'est pas un mécanisme figé du protocole.

### Mises à jour du protocole

Les mises à jour du protocole sont déployées via un mécanisme de **versioning on-chain avec activation à hauteur de bloc planifiée** :

- **Patch** (correctif) : 7 jours d'annonce avant activation
- **Minor** (nouvelle fonctionnalité) : 30 jours d'annonce
- **Major** (changement structurel) : 90 jours d'annonce

Ce mécanisme garantit que tous les opérateurs de nœuds ont le temps de se mettre à jour avant l'activation, évitant tout hard fork involontaire.

---

## 8. Règles Immuables

Les éléments suivants sont les **piliers de conception** de VinX :

1. **Cap de 100 milliards** de VinX (jamais augmenté)
2. **Aucun burn** — la supply est conservée pour toujours
3. **Invariant de la Fonderie** : `circulation + Fonderie = 100 Md` à chaque bloc
4. **Cycle melt/forge** : les frais fondent dans La Fonderie, les récompenses en sont forgées
5. **Consensus permissionné** (pas de switch vers PoW anonyme ou PoS ouvert)
6. **Propriété inconditionnelle des comptes** (aucun gel)

La conservation de la supply repose sur une **arithmétique entièrement *checked*** (aucun overflow/underflow silencieux) et une **finalité déterministe** (pas de réorganisation), et est couverte par des tests de propriété (`proptest`).

---

## 9. Positionnement

VinX Ledger est un **projet personnel et artisanal** : une monnaie souveraine, construite en solo, sans investisseurs ni pré-vente.

- **Aucun KYC au protocole** — la transparence on-chain est native.
- **Propriété inconditionnelle** — aucun compte ne peut être gelé.
- **Pas de portail commercial** ni de statut réglementaire visé à ce stade — VinX avance comme un bijou technique que son créateur affine dans le temps.

Un éventuel cadre de conformité pourra être étudié le jour où un usage public élargi le justifierait ; il n'est pas un prérequis du protocole.

---

## 10. Roadmap

Sans calendrier engagé, par étapes :

- **Étape actuelle** : le protocole (L1 Rust, consensus PoA Threshold, Fonderie melt/forge) est complet et testé, exploité en local.
- **Ensuite** : redondance multi-validateurs (1 → 3), distribution du milliard fondateur à un premier cercle, micro-économie réelle.
- **Plus tard (optionnel)** : réseau public, et de nouvelles briques que la version présente garde ouvertes (ex. *token factory* pour émettre d'autres actifs sur VinX).

VinX Ledger n'a pas de pression d'agenda. Le projet avance à son rythme.

---

## Annexe : Synthèse en une page

| Catégorie | Valeur |
|-----------|--------|
| Type | L1 indépendante, non-EVM, account-based |
| Stack | Rust, implémentation propriétaire |
| Cryptographie | Ed25519, SHA-256, Bech32 (`vinx1`) |
| Cadence de bloc | Adaptative : repos→0 · normal→~5s · charge→écart suit le remplissage · saturation→dos à dos |
| Capacité | 10 000 tx/bloc (réglable) · mempool 100 000 |
| TPS | Plusieurs milliers (config-dépendant) |
| Consensus | PoA Threshold, seuil 66%, finalité déterministe |
| Validateurs | 1 → 3 (redondance), permissionnés |
| Full nodes | Ouverts à tous |
| Supply totale | 100 milliards VinX (immuable, no burn) |
| Adresse | `[u8;20]` (SHA-256(pubkey)[..20]), affichée en bech32 `vinx1…` |
| Genèse | 1 Md (1 %) au fondateur, 99 Md (99 %) dans La Fonderie |
| Modèle monétaire | Melt/forge — invariant `circulation + Fonderie = 100 Md` |
| Frais | 0,05% + floor 0,0001 VinX, **100 % melt** dans La Fonderie |
| Staking | 1 VinX min, récompenses forgées /100 blocs, pas de slash |
| Burn | Aucun (jamais) — le métal cycle à l'infini |
| Gouvernance | Clé admin unique rotatable (fondateur), pas de gel de compte |
| Upgrades | Versioning on-chain, activation à hauteur planifiée, 7/30/90j |

---

*VinX Labs, juillet 2026 — Document de référence v3.1*
