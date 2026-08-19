# VinX Ledger — Livre Blanc

**Version :** 5.0
**Date :** Août 2026
**Éditeur :** VinX Labs

---

## 1. Vision & Philosophie

VinX Ledger est une **monnaie numérique artisanale**, conçue comme une infrastructure de paiement du quotidien, **fiable et précise au centime près**.

Elle refuse la spéculation et la complexité des smart contracts pour se concentrer sur une promesse simple : un **cash numérique honnête, rapide et souverain**.

Construit en solo, sans investisseurs, **sans pre-mine**, sans promesses spéculatives, VinX Ledger est une infrastructure financière qui assume son rythme et ses choix.

Cohérent avec cette philosophie, VinX est développé sur un **protocole Rust entièrement maîtrisé**, sans framework tiers imposant sa vision. Chaque ligne de code correspond exactement à ce que VinX veut être — rien de plus.

> **Ce qui change en v5.0 :** VinX abandonne le modèle « La Fonderie » (pré-allocation de 100 Md à la genèse, melt/forge) au profit d'un **minting progressif pur** : les tokens n'existent pas avant d'être produits. La courbe d'émission est allongée (~20 ans de demi-vie au lieu de 8) pour réduire le front-loading. Le slash est **redistribué aux validateurs honnêtes** via le pot d'époque — les tokens ne quittent jamais la circulation. Les modules gagnent un mécanisme de **rémunération par escrow on-chain** (ADR 0039). L'émission reste entièrement par le travail du consensus, sans pre-mine, sans robinet discrétionnaire.

> **📍 Direction décidée (post-v4.0, en cours de spécification/implémentation) — l'écosystème de subnets.** Ce document décrit l'économie **implémentée aujourd'hui** (fair launch, émission progressive à demi-vie ~20 ans). La direction actée pour la suite : remplacer le halving par une **émission élastique à réservoir** `E = r·F` (le *melt* recycle vers la Fonderie → l'émission s'auto-régule pour égaler l'usage), et employer cette émission à **financer des subnets** — des surcouches où l'on rend un service réel payé en VINX, l'émission étant dirigée **au prorata de l'usage réel** vers les fournisseurs, sans staking spéculatif. C'est ce qui donne à VinX sa proposition de valeur au-delà du paiement pur. Cette émission est **plafonnée par l'usage réel** (`min(r·F·Δt, k·M)`) pour empêcher un jackpot de démarrage à froid, suit un **canal unique demand-pull** (la valeur = ce que le client paie, jamais jugée par la chaîne), et combine **deux rails** (paiement direct prévisible + subvention-melt décroissante), avec l'invariant gravé **`CAP·k < 1`** qui rend l'auto-dealing non rentable. Dans cette direction, **les validateurs vivent des frais (fee-only PoA)** et l'émission va aux **subnets**, pas aux validateurs ; et comme des jetons doivent exister pour amorcer, **la formule « aucun jeton à la genèse » est remplacée par un *seed float modeste gagné par le travail*** (phase de bootstrap « subnet 0 », puis demand-pull) — l'immense majorité restant distribuée par le travail via les subnets. Détails : ADR [0047](docs/adr/0047-emission-elastique-reservoir.md), [0041](docs/adr/0041-repartition-emission-usage-melt.md), [0039](docs/adr/0039-infrastructure-subnets-escrow-recompense.md), [0042](docs/adr/0042-epoque-reglement-emission.md) (époque de règlement), [0044](docs/adr/0044-garde-fous-equite-amorcage-emission.md) (garde-fous d'équité, modèle & amorçage) ; idées de subnets : [catalogue](docs/subnets/CATALOGUE.md). Tant que ces ADR ne sont pas implémentés, **le modèle en vigueur reste celui décrit ci-dessous.**

---

## 2. Architecture Technique

- **Langage** : Rust, implémentation propriétaire de bout en bout
- **Vitesse** : Cadence de bloc fixe à **12 s** (ADR 0043) — un bloc produit toutes les 12 secondes, même vide. Finalité déterministe **au quorum** (prefix-closed) via le consensus **PoS Algorand-style** (comité VRF n ≈ 100, BLS agrégé) — immédiate à validateur unique, elle suit les co-signatures à n≥2
- **Capacité** : jusqu'à **3 000 transactions par bloc** (réglable), mempool de **100 000** transactions
- **Performance** : ~250 TPS (3 000 tx à 12 s ; l'exécution est séquentielle — une exécution parallèle serait requise pour dépasser)
- **Précision** : 18 décimales internes, 2 décimales affichées à l'utilisateur
- **Adresses** : Format Bech32 avec préfixe `vinx1`
- **Cryptographie** : Ed25519 (signatures), BLS12-381 (co-signatures agrégées), SHA-256 (hachage), Bech32 (adresses)
- **Référence de temps** : le **timestamp des blocs** (temps réel), pas la hauteur — voir §7. Toutes les garanties temporelles (émission, déliaison de bond, préavis d'upgrade) s'expriment en secondes, jamais en nombre de blocs (indépendant de la cadence).

### Choix du protocole custom

VinX Ledger est implémenté sans framework blockchain tiers. Ce choix garantit :

- **Auditabilité maximale** — surface de code réduite, aucune dépendance opaque
- **Maîtrise totale du protocole** — chaque règle est écrite explicitement, rien n'est hérité par défaut
- **Alignement avec la vision** — une monnaie artisanale mérite une implémentation artisanale
- **Stabilité à long terme** — aucune dépendance upstream susceptible de casser l'API

Les briques P2P (libp2p Rust), consensus PoA Threshold et mises à jour forkless sont développées nativement dans le projet.

---

## 3. Tokenomics — Fair launch & émission par le travail

La supply totale est fixée à **100 000 000 000 VinX** (100 milliards), **immuable**. Les tokens ne sont **jamais créés au-delà** de ce plafond. Le slash et le reaping ne détruisent pas les tokens (seul le dust de reaping, infime, est retiré) — la supply totale en circulation approche 100 Md de façon monotone.

### 3.1 Aucun pre-mine, aucune réserve pré-allouée

À la genèse :

```
Tokens émis       = 0 VinX   (0 %)
Circulation       = 0 VinX   (0 %)
```

**Rien n'existe au démarrage.** Pas de réserve, pas de Fonderie, pas d'allocation — même latente. Les VINX n'existent que lorsqu'ils sont mintés par le travail du consensus. Personne ne détient de VINX au bloc 0, pas même le fondateur : il obtiendra ses jetons exactement comme tout le monde, **en faisant tourner des validateurs**.

C'est le sens profond du choix. Faire tourner les premiers validateurs est un travail réel et risqué (infra, disponibilité, maintenance) ; ce travail est rémunéré par l'émission. Un « don » au fondateur serait redondant — il gagne ses jetons en portant le réseau, pas en se les attribuant.

### 3.2 L'émission : décroissance exponentielle continue, demi-vie ~20 ans

Les VINX sont **mintés progressivement** pour rémunérer la production de blocs. Le débit d'émission décroît de façon **continue et régulière** depuis le premier bloc :

```
R(t) = R₀ · e^(−λt)     avec  λ = ln(2) / T_half
                               T_half ≈ 20 ans (demi-vie)
                               R₀ ≈ 3,47 milliards VinX / an
```

L'intégrale sur l'infini vaut exactement **100 milliards** : toute la supply finit par être mintée, de plus en plus lentement, jusqu'à la poussière. Il n'y a **aucun événement discret** (pas de « halving-day » comme Bitcoin) — la courbe décroît en permanence, imperceptiblement à l'échelle humaine. Elle est **calculée sur le temps réel** (timestamps des blocs) : un réseau au repos ne minte rien, et le bloc qui le réveille encaisse l'émission accumulée pendant toute la période creuse.

| Échéance | Cumul émis | Restant à minter |
|---|---|---|
| Genèse | 0 | 100 Md |
| 20 ans | 50 Md | 50 Md |
| 40 ans | 75 Md | 25 Md |
| 66 ans | 90 Md | 10 Md |
| ~133 ans | 99 Md | ~1 Md |
| ∞ | → 100 Md | → poussière |

Avec une demi-vie de 20 ans, l'émission démarre plus basse (R₀ ≈ 3,47 Md/an contre 8,66 dans la v4) et se répartit sur une période bien plus longue — **réduisant structurellement la concentration early** sans toucher la masse totale.

### 3.3 Deux propriétés clés de l'émission

- **Distribuée par époque, jamais pondérée par le bond.** L'émission accumulée sur une fenêtre temporelle (époque) est distribuée entre proposeurs et co-signataires : les proposeurs reçoivent une fraction fixe (`PROPOSER_SHARE_BPS`) proportionnelle à leurs blocs dans l'époque ; le reste est réparti proportionnellement aux co-signatures valides. **Le montant du bond ne multiplie pas les gains** : seul le travail effectif compte (blocs proposés et co-signés). VinX refuse la logique plutocratique du « plus je stake, plus je gagne ».
- **En temps réel.** L'émission entre deux blocs est l'intégrale du débit sur l'intervalle `[timestamp précédent, timestamp courant]`. Si le réseau reste au repos, le bloc qui le réveille encaisse l'émission accumulée pendant toute la période creuse — on rémunère le temps réel écoulé **sans** produire de blocs vides au repos.

### 3.4 Le relais automatique vers les frais

Le revenu d'un validateur est la somme de deux flux : l'**émission** (dominante au début) et les **frais de transaction** (croissants avec l'usage). La courbe d'émission décroît continûment ; les frais croissent avec l'adoption. Quand l'émission atteint la poussière, les frais deviennent la source de revenu dominante et le réseau bascule en **fees-only**, pour toujours.

Ce relais se fait **tout seul**, sans intervention ni décision. L'incitation à sécuriser la chaîne existe dès le premier bloc et ne dépend jamais d'un robinet à couper à la main.

### 3.5 L'invariant fondateur

À **chaque bloc**, sans exception :

```
circulation + pot_époque + escrows_en_cours + poussière_détruite
    = émis_total  ≤  100 000 000 000 VinX
```

- `émis_total` croît continûment selon la courbe, jamais au-delà du cap.
- `pot_époque` = émission + slash redistribué, en attente de distribution à la clôture de l'époque.
- `escrows_en_cours` = paiements de modules bloqués en attente de preuve de livraison.
- `poussière_détruite` = dust des comptes reaped (≤ 0,001 VINX par compte, infime).

Forme simplifiée pour la communication : **la supply en circulation ne peut qu'augmenter** (jusqu'au cap), car le slash redistribue sans détruire.

L'invariant est vérifié à l'exécution (arithmétique *checked*, échec du bloc en cas de rupture), pas seulement en test.

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

### 4.2 Frais au producteur, émission par époque

**L'intégralité des frais est créditée immédiatement au validateur qui produit le bloc.** Les frais rémunèrent le travail d'**inclusion** (sélection des transactions, construction du bloc, infra) — une responsabilité qui appartient uniquement au proposeur.

L'**émission**, elle, rémunère la **sécurité collective** (co-signatures qui donnent la finalité). Elle est distribuée par **époque** (fenêtre temporelle, ex. 1 heure) entre proposeurs et co-signataires — proportionnellement à leur participation effective sur l'époque.

Le frais et l'émission **ne quittent jamais la circulation** — ils changent simplement de main. Il n'y a plus de *melt*, plus de réserve intermédiaire, plus de redistribution à des tiers passifs.

```
[Émission — par époque] ──► proposeurs + co-signataires ──► circulation
                                                              │
   ┌──────────────────────────────────────────────────────────┘
   ▼
[Frais forfaitaires — immédiats] ──► validateur producteur ──► circulation ──► …
```

---

## 5. Le staking — un bond de validateur, pas un rendement

Dans un réseau **PoS permissionless**, la sécurité vient du bond économique des validateurs et de la vérification cryptographique (ZK), **pas** d'une liste d'identités autorisées. Le staking n'a donc de sens que pour une chose : poser une **caution** — la peau dans le jeu qu'un validateur perd s'il triche.

- **Le staking retail est supprimé.** Il n'existe plus de « récompenses de staking » pour des détenteurs passifs. Un utilisateur lambda garde du VINX pour **l'utiliser comme cash**, point.
- **Bond minimum : `100 000 VinX`** (gouvernable) — requis pour être ajouté au set des validateurs. Le validateur défini à la genèse est **dispensé** (bootstrap : il démarre sans jeton et accumule son bond via l'émission).
- **Le bond ne rapporte aucun rendement** — c'est une garantie de sécurité, pas un placement. Le seul revenu d'un validateur vient de son **travail** (émission + frais).
- **Déliaison différée : `3 jours` de temps réel.** Quand un validateur retire son bond, les fonds ne reviennent pas immédiatement : ils traversent une période de **déliaison** (mesurée sur les timestamps, pas en blocs) pendant laquelle ils **restent saisissables**. Sans ce délai, un validateur pourrait tricher puis retirer sa caution avant que la preuve ne soit incluse.

### Slashing

- **Équivocation (double-signature)** : preuve cryptographiquement vérifiée — deux en-têtes de blocs distincts, à la même hauteur, portant deux signatures Ed25519 valides du même validateur. Sanction : **100 % du bond** (gouvernable), le validateur est exclu du set.
- **Répartition du slash** :
  - **10 %** → prime au **rapporteur** (pour rendre la surveillance rentable et inciter la vigilance).
  - **90 %** → versés dans le **pot d'époque** (ADR 0028), distribués aux validateurs honnêtes actifs à la clôture de l'époque, proportionnellement à leur participation. Le slash récompense collectivement ceux qui maintiennent le réseau sûr.
  - **Aucun token détruit** — le slash est une redistribution, pas une destruction. La supply en circulation reste inchangée à court terme.
- **Downtime** : un validateur hors-ligne au-delà d'un seuil est **suspendu** du round-robin (il ne produit plus, donc ne gagne plus) — mais **sans slash économique**, car l'absence n'est pas prouvablement malveillante.

---

## 6. Infrastructure : Validateurs & Full Nodes

### 6.1 Validateurs Core (PoA Threshold — Open PoA)

Nœuds qui produisent et co-signent les blocs, responsables de la sécurité du réseau.

**Rôle** : proposer et co-signer les blocs, maintenir le consensus, garantir la disponibilité du réseau.

**Mécanisme** : Proof of Authority Threshold — à chaque bloc, le validateur désigné (rotation déterministe) propose un bloc. Ce bloc est finalisé lorsque **plus de 66 % des validateurs actifs** l'ont co-signé. La finalité est déterministe : un bloc quorum-signé ne peut jamais être réorganisé.

**Tolérance aux pannes** : le réseau reste opérationnel tant que 66 % des validateurs sont en ligne.

**Admission — Open PoA :** à partir de la Phase 2, n'importe qui peut candidater en postant le bond requis — sans approbation admin individuelle. Les validateurs existants peuvent opposer un veto collectif (>66 %, fenêtre 7 jours). L'admin fixe seulement le montant du bond via gouvernance. Le set s'élargit en **trois phases automatiques et immuables**, gravées à la genèse :
- Phase 1 (bootstrap) : 3–5 validateurs, admission gouvernance-gated le temps d'éprouver le consensus.
- Phase 2 : 10–21 validateurs, Open PoA.
- Phase 3 : 50–101 validateurs, Open PoA.

**Rémunération** : par leur **travail** uniquement — l'émission distribuée par époque (proposeurs + co-signataires) puis les frais immédiats. Pour candidater, un validateur poste un **bond** (§5) ; ce bond le sécurise, il ne le rémunère pas.

**Score S_perf** : l'attribution des slots suit un score basé sur le taux de co-signature et de proposition réussie, 100 % déterministe et on-chain. Pas de délégation DPoS, pas de pondération par le bond.

### 6.2 Modules — Services hors-nœud ancrés et rémunérés

Un **module** est un service off-chain (stockage décentralisé, oracle, calcul, relai…) dont
l'opérateur poste un **bond VINX** pour s'enregistrer sur la L1. La L1 n'exécute jamais la
logique du module — elle ancre des **racines Merkle** prouvant l'état du service, et route
les **paiements** de façon déterministe.

**Cycle de vie d'un paiement de module :**

```
1. Client → ModuleEscrow (tx 0x0A) : bloque N VINX on-chain pour une commande de service.
2. Module livre le service off-chain.
3. Module ancre une preuve (AnchorState + EscrowRelease) : preuve Merkle de livraison.
4. L1 détecte la preuve → distribue atomiquement selon le fee_schedule du module :
      - Bénéficiaires enregistrés  (ex. 3 providers × 30 %)
      - Résidu à l'opérateur       (ex. 10 %)
5. Si pas de preuve avant timeout → client réclame le remboursement (ModuleEscrowRefund, tx 0x0B).
```

**Exemple concret** — module de stockage décentralisé, 100 Go, 100 VINX :

| Bénéficiaire | Part | Montant |
|---|---|---|
| Provider A | 30 % | 30 VINX |
| Provider B | 30 % | 30 VINX |
| Provider C | 30 % | 30 VINX |
| Opérateur (coordinateur) | 10 % | 10 VINX |

La structure interne du module (qui sont les providers, comment le coordinateur les rémunère)
est **entièrement off-chain** — la L1 ne voit que des adresses et des pourcentages. C'est un
**marché libre** : chaque module fixe son prix et sa structure de partage dans son
enregistrement. La concurrence entre modules régule naturellement les prix.

Le bond de l'opérateur est sa caution : un module qui ne livre pas répétitivement risque le
slashing (ADR 0023, à venir). Un timeout simple rembourse le client sans slash (distinction
entre défaut intentionnel prouvable et simple incident).

> **Aucune émission secondaire pour les modules.** Les modules sont rémunérés par leurs
> utilisateurs, pas par le protocole. VinX refuse les systèmes qui « force à transacter »
> pour capturer de l'émission — toute récompense protocolaire reste réservée au travail
> du consensus.

### 6.3 Full Nodes Communautaires

**N'importe qui peut faire tourner un nœud complet** sans permission et sans rémunération directe en Phase 1.

**Motivation triple** :
- **Conviction** : souveraineté financière et vérification indépendante
- **Utilité personnelle** : disposer d'un point d'accès RPC privé pour son propre usage
- **Auditabilité** : garantir collectivement que VinX Labs ne triche pas

Les full nodes sont **la couche de redondance** du réseau. Si VinX Labs disparaissait, les full nodes conservent l'intégralité de la blockchain et permettent au réseau de continuer d'exister.

---

## 7. Le temps dans VinX : timestamps, pas hauteur de bloc

La cadence de VinX étant **variable** (aucun bloc au repos, un plancher de 12 s sous charge), la hauteur de bloc **n'est pas une horloge** : le même nombre de blocs peut représenter quelques minutes d'activité ou un temps indéfini au repos. Toute garantie qui doit s'exprimer en temps réel s'appuie donc sur le **`timestamp` des en-têtes de blocs** :

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
2. **Quasi-absence de burn** — le slash redistribue aux validateurs honnêtes (ne détruit pas) ; seule la poussière des comptes reaped est détruite (≤ 0,001 VINX par compte, par construction)
3. **Aucun pre-mine, aucune réserve pré-allouée** — à la genèse, émis = 0 ; 100 % de la supply mintée par le travail des validateurs
4. **Invariant** : `circulation + pot_époque + escrows + poussière_détruite = émis ≤ 100 Md` à chaque bloc
5. **Émission décroissante continue puis relais aux frais** — jamais un robinet discrétionnaire ; jamais d'événement discret
6. **Le bond sécurise, le travail rémunère** — le stake ne produit aucun rendement ; le slash punit et récompense collectivement les honnêtes
7. **Consensus PoS natif avec comité VRF** (tirage uniforme parmi les validateurs bondés, comité n ≈ 100, finalité BFT déterministe — ADR 0029/0038)
8. **Propriété inconditionnelle des comptes** (aucun gel)
9. **La L1 n'exécute jamais de logique applicative** — les fonctionnalités complexes vivent dans des surcouches **hors-nœud**, reliées à VinX par ancrage bondé et rémunérées par escrow on-chain. Voir [ADR 0001](docs/adr/0001-l1-monnaie-pure-modules-ancrage-bonde.md) et [ADR 0039](docs/adr/0039-remuneration-operateurs-modules.md).
10. **Courbe d'émission immuable après la genèse** — le total (100 Md), la demi-vie (~20 ans) et la forme exponentielle continue ne sont gouvernables par **personne** (ni admin, ni action de gouvernance). Nul ne décide de la création monétaire. Voir [ADR 0021](docs/adr/0021-immutabilite-emission.md) et [ADR 0040](docs/adr/0040-emission-progressive-sans-fonderie.md).

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
- **Prochaine grande direction (décidée, à implémenter) — l'écosystème de subnets.** Le halving discret laisse place à une **émission élastique à réservoir** `E = r·F` (le *melt* recycle vers la Fonderie, auto-régulation vers un équilibre où l'émission égale l'usage) ; cette émission **finance des subnets** — des surcouches où des participants rendent un service réel (stockage, calcul, aléa…) payé en VINX — répartie **au prorata de l'usage réel (VINX melté)** par subnet, sans staking spéculatif à la TAO. Infrastructure : module bondé + **escrow** + **racine de récompense** + **Claim par preuve Merkle**. ADR [0047](docs/adr/0047-emission-elastique-reservoir.md) / [0041](docs/adr/0041-repartition-emission-usage-melt.md) / [0039](docs/adr/0039-infrastructure-subnets-escrow-recompense.md) ; s'appuie sur l'ancrage bondé de l'[ADR 0001](docs/adr/0001-l1-monnaie-pure-modules-ancrage-bonde.md).
- **Plus tard** : réseau public, décentralisation à l'échelle (agrégation BLS + comité VRF).

VinX Ledger n'a pas de pression d'agenda. Le projet avance à son rythme.

---

## Annexe : Synthèse en une page

| Catégorie | Valeur |
|-----------|--------|
| Type | L1 indépendante, non-EVM, account-based |
| Stack | Rust, implémentation propriétaire |
| Cryptographie | Ed25519, BLS12-381, SHA-256, Bech32 (`vinx1`) |
| Cadence de bloc | Fixe **12 s** (ADR 0043) — un bloc toutes les 12 s, même vide (pas de skip-empty ; congestion via base-fee) |
| Référence de temps | Timestamp des blocs (temps réel), pas la hauteur |
| Capacité | 3 000 tx/bloc (réglable) · ~250 TPS · mempool 100 000 |
| Consensus | **PoS Algorand-style** — comité VRF n ≈ 100, BLS agrégé, finalité BFT déterministe (ADR 0029) |
| Validateurs | **permissionless** — bond requis, admission libre, sélection par VRF (ADR 0038/0029) |
| Full nodes | Ouverts à tous |
| Supply totale | 100 milliards VinX (immuable, no burn) |
| **Genèse** | **0 émis, 0 en circulation — aucun pre-mine, aucune réserve pré-allouée** |
| **Émission** | **minting progressif par le travail · décroissance expo. continue, demi-vie ~20 ans → 100 Md · temps réel** |
| Relais | Émission → poussière → **fees-only** automatiquement |
| Frais | **forfait 0,0001 VinX × poids × congestion (×1–3), 100 % au producteur** |
| Staking | **bond de validateur (min 100k VinX), déliaison 3 j, slash équivocation 100 % (10 % rapporteur + 90 % redistribués aux validateurs honnêtes), aucun rendement** |
| Gouvernance | **Comité K-of-M** (multisig à seuil, ADR 0011) ou clé admin unique rotatable (legacy) ; pas de gel de compte |
| Upgrades | Versioning on-chain, activation planifiée, 7/30/90 j **réels** |

---

*VinX Labs, août 2026 — Document de référence v5.0*
