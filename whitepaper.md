# VinX Ledger — Livre Blanc

**Version :** 4.0
**Date :** Juillet 2026
**Éditeur :** VinX Labs

---

## 1. Vision & Philosophie

VinX Ledger est une **monnaie numérique artisanale**, conçue comme une infrastructure de paiement du quotidien, **fiable et précise au centime près**.

Elle refuse la spéculation et la complexité des smart contracts pour se concentrer sur une promesse simple : un **cash numérique honnête, rapide et souverain**.

Construit en solo, sans investisseurs, **sans pre-mine**, sans promesses spéculatives, VinX Ledger est une infrastructure financière qui assume son rythme et ses choix.

Cohérent avec cette philosophie, VinX est développé sur un **protocole Rust entièrement maîtrisé**, sans framework tiers imposant sa vision. Chaque ligne de code correspond exactement à ce que VinX veut être — rien de plus.

> **Ce qui change en v4.0 :** VinX abandonne le modèle *melt/forge* (frais fondus dans une réserve, récompensés à des stakers passifs) au profit d'un **fair launch** : aucun jeton n'est donné à la genèse, **tous les VINX entrent en circulation par le travail des validateurs**, et le staking redevient ce qu'il doit être dans un réseau permissionné — un **bond de sécurité**, pas un rendement.

> **📍 Direction décidée (post-v4.0, en cours de spécification/implémentation) — l'écosystème de subnets.** Ce document décrit l'économie **implémentée aujourd'hui** (fair launch, halving 8 ans). La direction actée pour la suite : remplacer le halving par une **émission élastique à réservoir** `E = r·F` (le *melt* recycle vers la Fonderie → l'émission s'auto-régule pour égaler l'usage), et employer cette émission à **financer des subnets** — des surcouches où l'on rend un service réel payé en VINX, l'émission étant dirigée **au prorata de l'usage réel** vers les fournisseurs, sans staking spéculatif. C'est ce qui donne à VinX sa proposition de valeur au-delà du paiement pur. Cette émission est **plafonnée par l'usage réel** (`min(r·F·Δt, k·M)`) pour empêcher un jackpot de démarrage à froid, suit un **canal unique demand-pull** (la valeur = ce que le client paie, jamais jugée par la chaîne), et combine **deux rails** (paiement direct prévisible + subvention-melt décroissante). Détails : ADR [0040](docs/adr/0040-emission-elastique-reservoir.md), [0041](docs/adr/0041-repartition-emission-usage-melt.md), [0039](docs/adr/0039-infrastructure-subnets-escrow-recompense.md), [0042](docs/adr/0042-epoque-reglement-emission.md) (époque de règlement), [0044](docs/adr/0044-garde-fous-equite-amorcage-emission.md) (garde-fous d'équité & amorçage) ; idées de subnets : [catalogue](docs/subnets/CATALOGUE.md). Tant que ces ADR ne sont pas implémentés, **le modèle en vigueur reste celui décrit ci-dessous.**

---

## 2. Architecture Technique

- **Langage** : Rust, implémentation propriétaire de bout en bout
- **Vitesse** : Cadence de bloc **adaptative à la demande** — *repos* → aucun bloc ; *activité normale* → jusqu'à ~5 s (block time), les transactions s'agrègent ; *montée en charge* → l'écart se resserre à mesure que le mempool se remplit ; *saturation* → blocs **dos à dos**. Finalité déterministe **au quorum** (prefix-closed) via le consensus PoA Threshold — immédiate à validateur unique, elle suit les co-signatures à n≥2
- **Capacité** : jusqu'à **10 000 transactions par bloc** (réglable), mempool de **100 000** transactions
- **Performance** : plusieurs milliers de TPS en configuration optimisée (dépend du matériel et des réglages ; l'exécution est séquentielle — une exécution parallèle serait requise au-delà)
- **Précision** : 18 décimales internes, 2 décimales affichées à l'utilisateur
- **Adresses** : Format Bech32 avec préfixe `vinx1`
- **Cryptographie** : Ed25519 (signatures), SHA-256 (hachage), Bech32 (adresses)
- **Référence de temps** : le **timestamp des blocs** (temps réel), pas la hauteur — voir §7. Toutes les garanties temporelles (émission, déliaison de bond, préavis d'upgrade) s'expriment en secondes, jamais en nombre de blocs, parce que la cadence est variable.

### Choix du protocole custom

VinX Ledger est implémenté sans framework blockchain tiers. Ce choix garantit :

- **Auditabilité maximale** — surface de code réduite, aucune dépendance opaque
- **Maîtrise totale du protocole** — chaque règle est écrite explicitement, rien n'est hérité par défaut
- **Alignement avec la vision** — une monnaie artisanale mérite une implémentation artisanale
- **Stabilité à long terme** — aucune dépendance upstream susceptible de casser l'API

Les briques P2P (libp2p Rust), consensus PoA Threshold et mises à jour forkless sont développées nativement dans le projet.

---

## 3. Tokenomics — Fair launch & émission par le travail

La supply totale est fixée à **100 000 000 000 VinX** (100 milliards), **immuable et sans burn**. Aucun jeton n'est jamais créé au-delà de ce plafond ni détruit en dessous : la valeur ne fait que **passer de la réserve à la circulation**, une fois, dans un seul sens — par le travail.

### 3.1 Aucun pre-mine

À la genèse :

```
La Fonderie (réserve d'émission) = 100 000 000 000 VinX   (100 %)
Circulation                       = 0 VinX                 (0 %)
```

**Personne ne détient de VINX au démarrage** — pas même le fondateur. Il n'y a **aucune allocation fondateur**, aucune vente privée, aucun jeton distribué à l'avance. Le fondateur obtiendra des VINX exactement comme tout le monde : **en faisant tourner des validateurs**.

C'est le sens profond du choix. Faire fonctionner les premiers validateurs est un travail réel et risqué (infra, disponibilité, maintenance) ; ce travail est rémunéré par l'émission. Un « don » au fondateur serait redondant — il gagne ses jetons en portant le réseau, pas en se les attribuant.

### 3.2 L'émission : décroissance exponentielle, halving tous les 8 ans

Les VINX sortent de La Fonderie **uniquement** pour rémunérer la production de blocs. Le débit d'émission décroît dans le temps et est **divisé par deux tous les 8 ans** :

```
débit(t) = R₀ · 2^(−t / 8 ans)          avec R₀ ≈ 8,66 milliards VinX / an
```

L'intégrale sur l'infini vaut exactement **100 milliards** : toute la supply finit par être émise, de plus en plus lentement. La courbe est **lisse** (pas de marche d'escalier au moment du halving) et **calculée sur le temps réel** (timestamps des blocs), de sorte qu'un réseau qui dort ou qui sature n'accélère ni ne fige l'émission.

| Échéance | Cumul émis | Restant dans La Fonderie |
|---|---|---|
| Genèse | 0 | 100 Md |
| 8 ans | 50 Md | 50 Md |
| 16 ans | 75 Md | 25 Md |
| 24 ans | 87,5 Md | 12,5 Md |
| … | → 100 Md | → poussière |

**L'émission est front-loadée à dessein** : elle est forte au début, quand l'économie est minuscule et que les frais ne suffisent pas à rémunérer les validateurs, puis elle s'efface à mesure que l'usage — et donc les frais — grandit.

### 3.3 Deux propriétés clés de l'émission

- **Créditée au producteur du bloc, jamais pondérée par le bond.** En rotation round-robin, chaque validateur produit environ 1 bloc sur *n* et encaisse donc ~1/*n* de l'émission sur la durée. **Le montant du bond ne multiplie pas les gains** : validateurs égaux, revenus espérés égaux. VinX refuse la logique plutocratique du « plus je stake, plus je gagne ».
- **En temps réel.** L'émission entre deux blocs est l'intégrale du débit sur l'intervalle `[timestamp précédent, timestamp courant]`. Si le réseau reste au repos, le bloc qui le réveille encaisse l'émission accumulée pendant toute la période creuse — on rémunère le temps réel écoulé **sans** produire de blocs vides au repos.

### 3.4 Le relais automatique vers les frais

Le revenu d'un validateur est la somme de deux flux : l'**émission** (dominante au début) et les **frais de transaction** (croissants avec l'usage). À mesure que La Fonderie se vide, l'émission s'efface et les frais deviennent la source de revenu dominante. Quand La Fonderie passe sous un seuil de poussière, l'émission s'arrête définitivement : le réseau bascule en **fees-only**, pour toujours.

Ce relais se fait **tout seul**, sans intervention ni décision. C'est la propriété la plus élégante du modèle : l'incitation à sécuriser la chaîne existe dès le premier jour et ne dépend jamais d'un robinet à couper à la main.

### 3.5 L'invariant fondateur

À **chaque bloc**, sans exception :

```
circulation + Fonderie = 100 000 000 000 VinX   (constant, pour toujours)
```

La Fonderie ne fait que décroître, la circulation ne fait que croître, leur somme est constante. L'invariant est vérifié à l'exécution (arithmétique *checked*, échec du bloc en cas de rupture), pas seulement en test.

---

## 4. Cycle Économique & Frais

### 4.1 Des frais forfaitaires, pas un pourcentage

Chaque transaction paie un **frais forfaitaire en valeur absolue**, **indépendant du montant transféré** :

```
frais = FRAIS_BASE × poids(type) × multiplicateur_congestion
```

- **`FRAIS_BASE = 0,0001 VinX`** (gouvernable) — un centième de centime. Le coût de traiter une transaction (vérifier une signature, l'appliquer, la stocker) ne dépend pas de la somme transportée : envoyer 1 VINX ou 1 million coûte **la même chose**. C'est le comportement attendu d'un vrai cash.
- **`poids(type)`** — un **transfert** pèse `1` ; le **stake / unstake** est **exempt** (`0`) — le bond a déjà un coût d'immobilisation, et le nombre de déliaisons par compte est plafonné contre le spam (ADR 0009) ; les **actions de gouvernance** (admin) sont **exemptes** (`0`).
- **`multiplicateur_congestion`** — façon EIP-1559 : `×1` jusqu'à 80 % de remplissage du mempool, montée linéaire jusqu'à `×3` à saturation. Basé sur la **demande**, jamais sur la valeur.

> VinX abandonne l'ancien frais *ad valorem* (0,05 % du montant), qui taxait injustement les gros paiements légitimes sans justification technique.

### 4.2 100 % au validateur producteur

**L'intégralité du frais est créditée au validateur qui produit le bloc.** Ce sont les seuls acteurs qui effectuent un travail réel (produire et co-signer les blocs, faire tourner l'infra) ; les payer directement est le modèle le plus honnête.

Le frais **ne quitte jamais la circulation** — il change simplement de main, de l'expéditeur vers le validateur. Il n'y a plus de *melt*, plus de réserve intermédiaire, plus de redistribution à des tiers passifs.

```
[Émission] ──(travail des validateurs)──► circulation ──► paiements P2P
                                                              │
   ┌──────────────────────────────────────────────────────────┘
   ▼
[Frais forfaitaires] ──► validateur producteur ──► circulation ──► …
```

---

## 5. Le staking — un bond de validateur, pas un rendement

Dans un réseau **PoA permissionné**, la sécurité vient de l'identité légale des validateurs et du seuil de co-signatures, **pas** d'un jeton. Le staking n'a donc de sens que pour une chose : poser une **caution** — la peau dans le jeu qu'un validateur perd s'il triche.

- **Le staking retail est supprimé.** Il n'existe plus de « récompenses de staking » pour des détenteurs passifs. Un utilisateur lambda garde du VINX pour **l'utiliser comme cash**, point.
- **Bond minimum : `100 000 VinX`** (gouvernable) — requis pour être ajouté au set des validateurs. Le validateur défini à la genèse est **dispensé** (bootstrap : il démarre sans jeton et accumule son bond via l'émission).
- **Le bond ne rapporte aucun rendement** — c'est une garantie de sécurité, pas un placement. Le seul revenu d'un validateur vient de son **travail** (émission + frais).
- **Déliaison différée : `3 jours` de temps réel.** Quand un validateur retire son bond, les fonds ne reviennent pas immédiatement : ils traversent une période de **déliaison** (mesurée sur les timestamps, pas en blocs) pendant laquelle ils **restent saisissables**. Sans ce délai, un validateur pourrait tricher puis retirer sa caution avant que la preuve ne soit incluse.

### Slashing

- **Équivocation (double-signature)** : preuve cryptographiquement vérifiée — deux en-têtes de blocs distincts, à la même hauteur, portant deux signatures Ed25519 valides du même validateur. Sanction : **100 % du bond** (gouvernable), le validateur est exclu du set.
- **Répartition du slash** : **10 %** de prime au rapporteur (pour rendre la surveillance rentable), le reste **fondu dans La Fonderie**.
- **Downtime** : un validateur hors-ligne au-delà d'un seuil est **suspendu** du round-robin (il ne produit plus, donc ne gagne plus) — mais **sans slash économique**, car l'absence n'est pas prouvablement malveillante.

---

## 6. Infrastructure : Validateurs & Full Nodes

### 6.1 Validateurs Core (PoA Threshold)

Liste restreinte de nœuds sélectionnés, opérés par VinX Labs et des partenaires de confiance, **légalement identifiés et responsables**.

**Rôle** : proposer et co-signer les blocs, maintenir le consensus, garantir la disponibilité du réseau.

**Mécanisme** : Proof of Authority Threshold — à chaque bloc, le validateur désigné (rotation déterministe) propose un bloc. Ce bloc est finalisé lorsque **plus de 66 % des validateurs actifs** (≥ 14 sur 21 à maturité) l'ont co-signé. La finalité est déterministe : un bloc quorum-signé ne peut jamais être réorganisé.

**Tolérance aux pannes** : le réseau reste opérationnel tant que 66 % des validateurs sont en ligne. Jusqu'à 33 % de validateurs hors-ligne ou défaillants sont tolérés sans interruption.

**Rémunération** : par leur **travail** uniquement — l'émission (forte au début) puis les frais (dominants à terme). Pour rejoindre le set, un validateur doit poser un **bond** (§5) ; ce bond le sécurise, il ne le rémunère pas.

**Évolution** : 1 (local) → 3 (redondance), extensible ensuite.

### 6.2 Full Nodes Communautaires

**N'importe qui peut faire tourner un nœud complet** sans permission et sans rémunération directe en Phase 1.

**Motivation triple** :
- **Conviction** : souveraineté financière et vérification indépendante
- **Utilité personnelle** : disposer d'un point d'accès RPC privé pour son propre usage
- **Auditabilité** : garantir collectivement que VinX Labs ne triche pas

Les full nodes sont **la couche de redondance** du réseau. Si VinX Labs disparaissait, les full nodes conservent l'intégralité de la blockchain et permettent au réseau de continuer d'exister.

---

## 7. Le temps dans VinX : timestamps, pas hauteur de bloc

La cadence de VinX étant **adaptative à la demande**, la hauteur de bloc **n'est pas une horloge** : le même nombre de blocs peut représenter quelques minutes en saturation ou un temps indéfini au repos. Toute garantie qui doit s'exprimer en temps réel s'appuie donc sur le **`timestamp` des en-têtes de blocs** :

- **L'émission** décroît selon le temps réel écoulé (§3.2). *(implémenté)*
- **La déliaison de bond** mûrit après 3 jours réels (§5). *(implémenté)*
- **Les préavis d'upgrade** visent des jours réels (§8) — *actuellement encore comptés en hauteur de bloc ; migration vers les timestamps planifiée.*

Pour empêcher un producteur malhonnête de gonfler le temps, chaque bloc est validé contre des **bornes de timestamp** : monotonie non-décroissante (`≥` celui du bloc précédent) et plafond (`≤` horloge locale + petite tolérance).

---

## 8. Gouvernance & Évolution

La gouvernance est assurée par une **clé admin unique** (le fondateur), **rotatable à chaud** via `AdminAction::RotateAdmin` sans redémarrage du nœud. Ses seules prérogatives on-chain : gérer le set de validateurs, ajuster les paramètres gouvernables (frais de base, bond minimum), et planifier les mises à jour de protocole.

Il n'y a **pas de gel de compte** : la propriété des jetons est inconditionnelle.

> Une répartition du contrôle admin (signature à seuil / multi-parties) pourra être introduite plus tard si le réseau grandit — ce n'est pas un mécanisme figé du protocole.

### Mises à jour du protocole

Les mises à jour sont déployées via un **versioning on-chain avec activation planifiée** :

- **Patch** (correctif) : 7 jours d'annonce avant activation
- **Minor** (nouvelle fonctionnalité) : 30 jours d'annonce
- **Major** (changement structurel) : 90 jours d'annonce

Ces préavis **visent des jours réels** afin que tous les opérateurs aient le temps de se mettre à jour et d'éviter tout hard fork involontaire. *Note d'implémentation : ils sont aujourd'hui encore appliqués en hauteur de bloc ; leur passage aux timestamps — comme l'émission et la déliaison — est planifié (cf. `ETAT_DU_PROJET.md` §8).*

---

## 9. Règles Immuables

Les éléments suivants sont les **piliers de conception** de VinX :

1. **Cap de 100 milliards** de VinX (jamais augmenté)
2. **Aucun burn** — la supply est conservée pour toujours
3. **Aucun pre-mine** — 100 % de la supply émise par le travail des validateurs
4. **Invariant** : `circulation + Fonderie = 100 Md` à chaque bloc
5. **Émission décroissante puis relais aux frais** — jamais un robinet discrétionnaire
6. **Le bond sécurise, le travail rémunère** — le stake ne produit aucun rendement
7. **Consensus permissionné** (pas de switch vers PoW anonyme ou PoS ouvert)
8. **Propriété inconditionnelle des comptes** (aucun gel)
9. **La L1 n'exécute jamais de logique applicative** — les fonctionnalités complexes vivent dans des surcouches **hors-nœud**, reliées à VinX par ancrage bondé (un hash + un bond + des transferts VINX). Voir [ADR 0001](docs/adr/0001-l1-monnaie-pure-modules-ancrage-bonde.md).
10. **Courbe d'émission immuable** — le total (100 Md), la période de halving (8 ans) et la forme de la courbe ne sont gouvernables par **personne** (ni admin, ni action de gouvernance). Nul ne décide de la création monétaire. Voir [ADR 0021](docs/adr/0021-immutabilite-emission.md).

La conservation de la supply repose sur une **arithmétique entièrement *checked*** (aucun overflow/underflow silencieux) et une **finalité au quorum** (un bloc co-signé par le quorum n'est pas réorganisé), couvertes par des tests de propriété (`proptest`).

---

## 10. Positionnement

VinX Ledger est un **projet personnel et artisanal** : une monnaie souveraine, construite en solo, sans investisseurs ni pré-vente.

- **Aucun pre-mine, aucune allocation privilégiée** — le fondateur gagne ses VINX par le travail comme tout le monde.
- **Aucun KYC au protocole** — la transparence on-chain est native.
- **Propriété inconditionnelle** — aucun compte ne peut être gelé.
- **Pas de portail commercial** ni de statut réglementaire visé à ce stade — VinX avance comme un bijou technique que son créateur affine dans le temps.

Un éventuel cadre de conformité pourra être étudié le jour où un usage public élargi le justifierait ; il n'est pas un prérequis du protocole.

---

## 11. Roadmap

Sans calendrier engagé, par étapes :

- **Fait** : le protocole (L1 Rust, consensus PoA Threshold) et le modèle *fair launch* décrit ici (émission par le travail, bond de validateur avec slashing prouvable, frais au producteur) sont **implémentés et testés**. Le **consensus multi-validateur est éprouvé au banc n=3** (`scripts/bench-n3.sh`) : finalité au quorum, tolérance à 1 panne, sûreté à 1/3 ; jailing/rotation sur set actif et fork-choice déterministe (fonction pure) en place.
- **En cours (consensus)** : wiring reorg du fork-choice, tx d'unjail, accountability des co-signatures conflictuelles.
- **Prochaine grande direction (décidée, à implémenter) — l'écosystème de subnets.** Le halving discret laisse place à une **émission élastique à réservoir** `E = r·F` (le *melt* recycle vers la Fonderie, auto-régulation vers un équilibre où l'émission égale l'usage) ; cette émission **finance des subnets** — des surcouches où des participants rendent un service réel (stockage, calcul, aléa…) payé en VINX — répartie **au prorata de l'usage réel (VINX melté)** par subnet, sans staking spéculatif à la TAO. Infrastructure : module bondé + **escrow** + **racine de récompense** + **Claim par preuve Merkle**. ADR [0040](docs/adr/0040-emission-elastique-reservoir.md) / [0041](docs/adr/0041-repartition-emission-usage-melt.md) / [0039](docs/adr/0039-infrastructure-subnets-escrow-recompense.md) ; s'appuie sur l'ancrage bondé de l'[ADR 0001](docs/adr/0001-l1-monnaie-pure-modules-ancrage-bonde.md).
- **Plus tard** : réseau public, décentralisation à l'échelle (agrégation BLS + comité VRF).

VinX Ledger n'a pas de pression d'agenda. Le projet avance à son rythme.

---

## Annexe : Synthèse en une page

| Catégorie | Valeur |
|-----------|--------|
| Type | L1 indépendante, non-EVM, account-based |
| Stack | Rust, implémentation propriétaire |
| Cryptographie | Ed25519, SHA-256, Bech32 (`vinx1`) |
| Cadence de bloc | Adaptative : repos→0 · normal→~5s · charge→écart suit le remplissage · saturation→dos à dos |
| Référence de temps | Timestamp des blocs (temps réel), pas la hauteur |
| Capacité | 10 000 tx/bloc (réglable) · mempool 100 000 |
| Consensus | PoA Threshold, quorum `⌈2n/3⌉` sur le set complet, finalité prefix-closed déterministe (éprouvée au banc n=3) |
| Validateurs | permissionnés, **bond requis** ; rotation leader/backup sur le set actif (jailing, ADR 0027) |
| Full nodes | Ouverts à tous |
| Supply totale | 100 milliards VinX (immuable, no burn) |
| **Genèse** | **0 en circulation, 100 Md en Fonderie — aucun pre-mine** |
| **Émission** | **par le travail des validateurs · halving 8 ans → 100 Md · temps réel** *(évolution décidée : émission élastique `E=r·F`, ADR 0040)* |
| Relais | Fonderie vidée → **fees-only** automatiquement |
| Frais | **forfait 0,0001 VinX × poids × congestion (×1–3), 100 % au producteur** |
| Staking | **bond de validateur (min 100k VinX), déliaison 3 j, slash équivocation 100 %, aucun rendement** |
| Gouvernance | **Comité K-of-M** (multisig à seuil, ADR 0011) ou clé admin unique rotatable (legacy) ; pas de gel de compte |
| Upgrades | Versioning on-chain, activation planifiée, 7/30/90 j **réels** |

---

*VinX Labs, août 2026 — Document de référence v4.0 (économie implémentée : fair launch/halving ; évolution décidée : émission élastique + subnets, ADR 0039/0040/0041)*
