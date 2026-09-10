# VinX Ledger — Livre Blanc

**Version :** 6.0
**Date :** Septembre 2026
**Éditeur :** VinX Labs

---

## 1. Vision & Philosophie

VinX Ledger est un **rail de paiement L1 minimaliste** : envoyer des VinX rapidement, de façon fiable et vérifiable. Point.

Ce n'est pas une plateforme de contrats intelligents. Ce n'est pas un L1 généraliste. Il n'y a pas de machine virtuelle, pas de modules applicatifs, pas de surcouches — et c'est un choix délibéré, pas un manque.

La valeur de VinX repose sur une promesse simple : un **cash numérique honnête, rapide et souverain**, dont chaque règle peut être lue, comprise et vérifiée par n'importe qui. Un protocole qui fait une chose et la fait bien, plutôt qu'un écosystème qui fait beaucoup et qu'on ne peut pas auditer complètement.

Construit en solo, sans investisseurs, **sans pre-mine**, sans promesses spéculatives, VinX avance à son rythme.

> **Ce qui change en v6.0 :** VinX abandonne l'écosystème de subnets / modules / Appchains ZK prévu dans les versions précédentes. L'architecture se recentre entièrement sur le rail de paiement. Les ADRs 0001, 0010, 0024, 0034, 0048, 0049, 0050 qui décrivaient ces fonctionnalités sont gelés hors scope (ADR 0064). La cryptographie de hachage passe de SHA-256 à BLAKE3 avant le genesis (ADR 0069).

---

## 2. Architecture Technique

- **Langage** : Rust, implémentation propriétaire de bout en bout — aucun framework blockchain tiers
- **Vitesse** : Cadence de bloc fixe à **6 s** — un bloc produit toutes les 6 secondes, même vide. Finalité déterministe **au quorum** via BLS agrégé
- **Capacité** : jusqu'à **3 000 transactions par bloc** (gouvernable), mempool de **100 000** transactions
- **Performance** : ~500 TPS (3 000 tx à 12 s) — exécution séquentielle intentionnelle pour l'auditabilité
- **Précision** : 18 décimales internes, 2 décimales affichées à l'utilisateur
- **Adresses** : Format Bech32 avec préfixe `vinx1`
- **Cryptographie** : Ed25519 (signatures de transaction), BLS12-381 (co-signatures agrégées du comité), BLAKE3 (hachage), Bech32 (adresses)
- **Référence de temps** : le **timestamp des blocs** (temps réel), pas la hauteur — toutes les garanties temporelles (émission, déliaison de bond, préavis d'upgrade) s'expriment en secondes

### Pourquoi une implémentation custom sans VM

VinX Ledger est implémenté sans framework blockchain tiers et sans machine virtuelle. Ce choix garantit :

- **Auditabilité maximale** — surface de code réduite, aucune dépendance opaque, chaque règle est écrite explicitement
- **Sécurité prévisible** — sans VM, la surface d'attaque est fixe et bornée ; aucun contrat malveillant ne peut s'exécuter dans le nœud
- **Maîtrise totale du protocole** — rien n'est hérité par défaut d'un framework
- **Stabilité à long terme** — aucune dépendance upstream susceptible de casser l'API

Les briques P2P (libp2p Rust), le consensus PoA avec comité VRF et BLS12-381, et les optimisations réseau (compact blocks ADR 0037, parallel sync ADR 0038) sont développées nativement dans le projet.

### Types de transactions

VinX reconnaît exactement cinq types de transactions — ni plus, ni moins :

| Type | Usage |
|------|-------|
| `Transfer` | Paiement natif VinX (le cas d'usage central) |
| `Bond` | Déposer le bond pour devenir validateur |
| `Unbond` | Retirer le bond (déliaison différée 3 jours) |
| `ValidatorJoin` / `ValidatorExit` | Rejoindre ou quitter le set de validateurs |
| `FeeAdjust` | Gouvernance des frais de base |

C'est tout. Il n'y a pas de transaction de contrat, pas de déploiement de code, pas d'appel à une logique applicative. Un nœud VinX n'exécute jamais de code tiers.

---

## 3. Tokenomics — Fair launch & émission par le travail

La supply totale est fixée à **100 000 000 000 VinX** (100 milliards), **immuable**. Les tokens ne sont jamais créés au-delà de ce plafond.

### 3.1 Aucun pre-mine, aucune réserve pré-allouée

À la genèse :

```
Tokens émis       = 0 VinX   (0 %)
Circulation       = 0 VinX   (0 %)
```

Rien n'existe au démarrage. Les VinX n'existent que lorsqu'ils sont mintés par le travail du consensus. Le fondateur lui-même obtient ses premiers tokens en faisant tourner des validateurs — pas autrement.

### 3.2 Émission : décroissance exponentielle continue, demi-vie ~20 ans

Les VinX sont mintés progressivement pour rémunérer la production de blocs. Le débit décroît de façon **continue et régulière** depuis le premier bloc :

```
R(t) = R₀ · e^(−λt)     avec  λ = ln(2) / T_half
                               T_half ≈ 20 ans
                               R₀ ≈ 3,47 milliards VinX / an
```

L'intégrale sur l'infini vaut exactement 100 milliards. Il n'y a **aucun événement discret** (pas de halving-day) — la courbe décroît en permanence. L'émission est calculée sur les **timestamps réels des blocs** : un réseau au repos ne minte rien.

| Échéance | Cumul émis | Restant |
|----------|-----------|---------|
| Genèse | 0 | 100 Md |
| 20 ans | 50 Md | 50 Md |
| 40 ans | 75 Md | 25 Md |
| 66 ans | 90 Md | 10 Md |
| ∞ | → 100 Md | → poussière |

### 3.3 Distribution de l'émission

L'émission accumulée sur une **époque** (fenêtre temporelle) est distribuée entre proposeurs et co-signataires :

- Les **proposeurs** reçoivent une fraction fixe (`PROPOSER_SHARE_BPS`) proportionnelle à leurs blocs dans l'époque.
- Le reste est réparti proportionnellement aux **co-signatures valides**.
- **Le montant du bond ne multiplie pas les gains** : seul le travail effectif compte.

### 3.4 Relais automatique vers les frais

Deux flux de revenus pour un validateur : l'**émission** (dominante au début, décroissante) et les **frais de transaction** (croissants avec l'usage). Quand l'émission atteint la poussière, les frais deviennent la source dominante et le réseau bascule en **fees-only** — automatiquement, sans intervention.

### 3.5 Invariant de supply

À chaque bloc, sans exception :

```
circulation + pot_époque + poussière_détruite = émis_total ≤ 100 000 000 000 VinX
```

- `émis_total` croît continûment selon la courbe, jamais au-delà du cap.
- `pot_époque` = émission + slash redistribué, en attente de distribution à la clôture.
- `poussière_détruite` = dust des comptes reaped (≤ 0,001 VinX par compte, infime).

L'invariant est vérifié à l'exécution (arithmétique *checked*, échec du bloc en cas de rupture).

---

## 4. Frais

### 4.1 Frais forfaitaires

Chaque transfert paie un frais **fixe en valeur absolue**, indépendant du montant transféré :

```
frais = FRAIS_BASE × poids(type) × multiplicateur_congestion
```

- **`FRAIS_BASE = 0,0001 VinX`** (gouvernable) — envoyer 1 VinX ou 1 million coûte la même chose. C'est le comportement attendu d'un vrai cash.
- **`poids(type)`** : transfert = `1` ; bond/unbond = `0` (exempté) ; actions de gouvernance = `0`.
- **`multiplicateur_congestion`** : `×1` jusqu'à 80 % de remplissage du mempool, montée linéaire jusqu'à `×3` à saturation.

### 4.2 Frais au producteur

L'intégralité des frais est créditée immédiatement au validateur qui produit le bloc. Les frais rémunèrent le travail d'inclusion — une responsabilité qui appartient uniquement au proposeur.

```
[Émission — par époque]      ──► proposeurs + co-signataires
[Frais forfaitaires — immédiats] ──► validateur producteur
```

Ni les frais ni l'émission ne quittent jamais la circulation — ils changent simplement de main.

---

## 5. Staking — un bond de validateur, pas un rendement

Le staking dans VinX a une seule fonction : **poser une caution**. Ce n'est pas un mécanisme de rendement pour les détenteurs passifs.

- **Pas de staking retail.** Un utilisateur lambda garde du VinX pour l'utiliser comme cash.
- **Bond minimum : `100 000 VinX`** (gouvernable) — requis pour rejoindre le set des validateurs. Le validateur du genesis est dispensé (il accumule son bond via l'émission).
- **Le bond ne rapporte aucun rendement** — c'est une garantie de sécurité. Le seul revenu d'un validateur vient de son travail (émission + frais).
- **Déliaison différée : 3 jours réels.** Les fonds retirés restent saisissables pendant la période de déliaison — sans ce délai, un validateur pourrait tricher puis retirer sa caution avant que la preuve ne soit traitée.

### Slashing

- **Équivocation (double-signature)** : preuve cryptographiquement vérifiée — deux en-têtes distincts, à la même hauteur, signés par le même validateur. Sanction : **100 % du bond**, exclusion du set.
  - **10 %** → prime au rapporteur (incite la vigilance).
  - **90 %** → versés dans le pot d'époque, distribués aux validateurs honnêtes.
  - **Aucun token détruit** — le slash est une redistribution.
- **Downtime** : exclusion du set actif à la prochaine rotation d'époque, **sans slash économique** — l'absence n'est pas prouvablement malveillante.

---

## 6. Infrastructure : Validateurs & Full Nodes

### 6.1 Validateurs (PoA avec comité VRF)

Nœuds qui produisent et co-signent les blocs, responsables de la sécurité du réseau.

**Mécanisme actuel (testnet) :** PoA — un comité de validateurs autorisés co-signent les blocs via **BLS12-381 agrégé**. Le leader est sélectionné par **ECVRF RFC 9381** (tirage imprévisible jusqu'au dernier moment — résistance DoS). Le bloc est finalisé lorsque **≥ 67 % du comité** l'ont co-signé. La finalité est BFT déterministe : un bloc quorum-signé ne peut jamais être réorganisé.

**Admission :** n'importe qui peut rejoindre le pool en postant le bond requis. Warmup de 3 époques avant d'être éligible. L'admin fixe seulement le montant du bond via gouvernance, dans des bornes immuables.

**Rémunération :** par le travail uniquement — émission d'époque + frais immédiats. Le bond sécurise, il ne rémunère pas.

**Tolérance aux pannes :** le réseau reste opérationnel tant que 67 % du comité sont en ligne.

### 6.2 Full Nodes Communautaires

N'importe qui peut faire tourner un nœud complet sans permission et sans rémunération directe.

**Motivations :** souveraineté financière, point d'accès RPC privé, auditabilité indépendante du réseau. Les full nodes sont la couche de redondance : si VinX Labs disparaissait, ils conservent l'intégralité de la blockchain.

**Synchronisation :** trois modes selon le retard — snapshot sync (ADR 0038, gap > 500 blocs), parallel sync (8 fetches concurrents, ADR 0038), puis sync séquentielle pour la queue finale.

---

## 7. Le temps dans VinX : timestamps, pas hauteur de bloc

Toutes les garanties qui s'expriment en temps réel reposent sur le **timestamp des en-têtes de blocs**, jamais sur la hauteur :

- L'**émission** décroît selon le temps réel écoulé (§3.2).
- La **déliaison de bond** mûrit après 3 jours réels (§5).
- Les **préavis d'upgrade** visent des jours réels (§8).

Pour empêcher un producteur malhonnête de gonfler le temps, chaque bloc est validé contre des bornes de timestamp : monotonie non-décroissante et plafond (horloge locale + tolérance).

---

## 8. Gouvernance & Évolution

La gouvernance est assurée par une **clé admin unique** (le fondateur), rotatable à chaud via `AdminAction::RotateAdmin` sans redémarrage. Ses prérogatives on-chain : gérer le set de validateurs, ajuster les paramètres gouvernables (frais de base, bond minimum), planifier les mises à jour.

Il n'y a **pas de gel de compte** : la propriété des tokens est inconditionnelle.

> Une répartition du contrôle admin (signature à seuil) pourra être introduite si le réseau grandit.

### Mises à jour du protocole

Les mises à jour sont déployées via un **versioning on-chain avec activation planifiée** :

- **Patch** (correctif) : 7 jours d'annonce
- **Minor** (nouvelle fonctionnalité) : 30 jours d'annonce
- **Major** (changement structurel) : 90 jours d'annonce

Ces préavis visent des **jours réels** pour que tous les opérateurs aient le temps de mettre à jour leurs nœuds.

---

## 9. Règles Immuables

Les piliers de conception de VinX, que nulle gouvernance ne peut modifier :

1. **Cap de 100 milliards** de VinX — jamais augmenté.
2. **Quasi-absence de burn** — le slash redistribue aux validateurs honnêtes, ne détruit pas. Seule la poussière des comptes reaped est détruite (≤ 0,001 VinX par compte).
3. **Aucun pre-mine, aucune réserve pré-allouée** — à la genèse, émis = 0. 100 % de la supply mintée par le travail.
4. **Invariant** : `circulation + pot_époque + poussière_détruite = émis ≤ 100 Md` à chaque bloc.
5. **Émission décroissante continue puis relais aux frais** — jamais un robinet discrétionnaire, jamais un événement discret.
6. **Le bond sécurise, le travail rémunère** — le stake ne produit aucun rendement ; le slash punit et récompense collectivement les honnêtes.
7. **Finalité BFT déterministe** — comité VRF, BLS agrégé, ≥ 67 % pour finaliser. Un bloc finalisé ne peut pas être réorganisé.
8. **Propriété inconditionnelle** — aucun compte ne peut être gelé.
9. **Aucune logique applicative dans le nœud** — VinX est un rail de paiement. Il n'exécute jamais de code tiers. Il n'y a pas de VM, pas de modules, pas de smart contracts. Ce périmètre est une décision de conception (ADR 0064), pas une limitation technique.
10. **Courbe d'émission immuable après la genèse** — le total (100 Md), la demi-vie (~20 ans) et la forme exponentielle continue ne sont gouvernables par personne.

---

## 10. Positionnement

VinX Ledger est un **projet artisanal** : une monnaie souveraine, construite en solo, sans investisseurs ni pré-vente.

**Ce que VinX est :**
- Un rail de paiement L1 — envoyer de la valeur vite, sûrement, de façon vérifiable.
- Un protocole minimaliste — auditable par une seule personne attentive.
- Un cash numérique — forfait de frais, précision au centime, finalité immédiate.

**Ce que VinX n'est pas :**
- Une plateforme de DeFi ou de smart contracts.
- Un concurrent d'Ethereum ou de Solana.
- Un système de rendement passif.

L'avantage de ce positionnement : VinX n'entre pas en compétition directe avec les L1 généralistes. Il résout un problème précis — le paiement — mieux qu'eux, parce qu'il ne fait que ça.

---

## 11. Roadmap

Sans calendrier engagé, par étapes :

**✅ Fait**
- Protocole core complet : consensus PoA + BLS, ECVRF RFC 9381, compact blocks (ADR 0037), snapshot sync + parallel sync (ADR 0038), storage persistant, RPC REST
- Modèle fair launch implémenté : émission par le travail, bond validateur, slashing prouvable, frais au producteur
- Consensus multi-validateur éprouvé (banc n=3 + banc adversarial multi-nœuds : finalité au quorum, tolérance à 1 panne, sûreté à 1/3)
- **BLAKE3** intégré avant toute genèse publique (ADR 0069) — remplace SHA-256, breaking assumé en pré-mainnet
- **Durcissement post-audit (ADRs 0069–0080)** : auth proposeur + registre BLS (0070), verrou de vote persistant (0071), `consensus_root` complet (0072), `signing_bytes` injectif (0073), checkpoints de subjectivité faible (0074), clé BLS + PoP liée à l'identité au bonding (0075)
- Infrastructure testnet : Dockerfile, docker-compose, genesis-testnet.json

**🔜 Prochaines étapes (dans l'ordre)**
1. **Genesis testnet** — fixer les adresses réelles, timestamp coordonné, lancer le nœud public
2. **HTTPS** sur le RPC public (nginx + Let's Encrypt)
3. **3+ validateurs indépendants** — cérémonie de genèse documentée (ADR 0075 §1), vérification du genesis hash, recoupement multi-pairs des checkpoints (ADR 0074)
4. **Contre-audits externes** + publication SECURITY.md (ADR 0078)

**📋 Backlog**
- Monitoring : Prometheus + Grafana sur `/metrics`
- Faucet public
- Wallet web minimaliste (balance, send, historique)
- Optimisations validées : mimalloc (ADR 0065), hash mémoïsé (ADR 0066), sig cache (ADR 0067), batch verify (ADR 0068)

VinX Ledger n'a pas de pression d'agenda. Le projet avance à son rythme.

---

## Annexe : Synthèse en une page

| Catégorie | Valeur |
|-----------|--------|
| **Type** | Rail de paiement L1 — non-EVM, account-based, sans VM |
| **Stack** | Rust, implémentation propriétaire |
| **Cryptographie** | Ed25519 (signatures), BLS12-381 (co-signatures), BLAKE3 (hachage), Bech32 `vinx1` |
| **Cadence de bloc** | Fixe **6 s** — un bloc toutes les 6 s, même vide |
| **Référence de temps** | Timestamp des blocs (temps réel), pas la hauteur |
| **Capacité** | 3 000 tx/bloc · ~500 TPS · mempool 100 000 |
| **Consensus** | PoA + comité ECVRF, BLS12-381 agrégé, finalité BFT déterministe (≥ 67 %) |
| **Types de tx** | Transfer, Bond, Unbond, ValidatorJoin, ValidatorExit, FeeAdjust — c'est tout |
| **Full nodes** | Ouverts à tous |
| **Supply totale** | 100 milliards VinX — immuable, no burn |
| **Genèse** | 0 émis, 0 en circulation — aucun pre-mine |
| **Émission** | Minting progressif · décroissance expo. continue · demi-vie ~20 ans · temps réel |
| **Relais** | Émission → poussière → **fees-only** automatiquement |
| **Frais** | Forfait 0,0001 VinX × poids × congestion (×1–3) · 100 % au producteur |
| **Staking** | Bond validateur (min 100k VinX) · déliaison 3 j · slash équivocation 100 % (10 % rapporteur + 90 % redistribués) · aucun rendement passif |
| **Gouvernance** | Clé admin rotatable · pas de gel de compte · upgrades planifiés 7/30/90 j réels |
| **Hors scope** | Smart contracts, VM, modules, Appchains, ZK, DeFi (ADR 0064) |

---

*VinX Labs, septembre 2026 — Document de référence v6.0*
