# VinX Ledger — Livre Blanc

**Version :** 2.2
**Date :** Juin 2026
**Éditeur :** VinX Labs

---

## 1. Vision & Philosophie

VinX Ledger est une **monnaie numérique artisanale**, conçue comme une infrastructure de paiement du quotidien, **fiable et précise au centime près**.

Elle refuse la spéculation et la complexité des smart contracts pour se concentrer sur une promesse simple : un **cash numérique honnête, rapide et souverain**.

Construit en solo, sans investisseurs, sans pre-mine dilué, sans promesses spéculatives, VinX Ledger est une infrastructure financière qui assume son rythme et ses choix.

---

## 2. Architecture Technique

- **Framework** : L1 indépendante développée en Rust via Substrate (Polkadot SDK)
- **Vitesse** : Blocs de 10 secondes, finalité déterministe immédiate via le consensus FBA (Aura + GRANDPA)
- **Performance** : Cible de 1 500 transactions par seconde (TPS)
- **Précision** : 18 décimales internes, 2 décimales affichées à l'utilisateur
- **Adresses** : Format Bech32 avec préfixe `vinx1`

---

## 3. Tokenomics & Les Deux Réserves

La supply totale est fixée à **100 000 000 000 VinX** (100 milliards).

Cette supply est techniquement créée au genesis, mais **structurée en deux compartiments on-chain strictement séparés** par des mécanismes cryptographiques.

### 3.1 L'Escrow "Sandbox" — Phase 1

**21 000 000 VinX** (0,021% du cap total)

Seule cette micro-fraction est active au lancement. Elle finance :

- Les contributions techniques (code, wallets, outils pour l'écosystème)
- Les programmes de bug bounty pour éprouver la sécurité
- Les premiers tests économiques (staking, transactions, échanges P2P)

Le chiffre de 21 millions est un clin d'œil assumé aux 21 millions de Bitcoin et Bittensor — symbole de la rareté et de la rigueur dans l'écosystème crypto.

### 3.2 Le Coffre "Maturité" — Phases ultérieures

**99 979 000 000 VinX** (99,979% du cap)

Cette réserve est **cryptographiquement verrouillée et inaccessible** par tout mécanisme courant du protocole. Elle ne peut être déverrouillée qu'avec **trois conditions cumulatives** :

1. **Statut CASP MiCA obtenu** par VinX Labs
2. **Audit indépendant validé** de la santé économique du réseau et de la robustesse technique
3. **Politique de distribution publique** publiée au moins 12 mois avant tout déblocage

Le Coffre Maturité n'a aucun calendrier engagé. Il peut rester verrouillé pendant des années ou des décennies si les conditions ne sont pas atteintes.

---

## 4. Distribution de la Sandbox

Les 21M VinX de la Sandbox sont **distribués manuellement par VinX Labs aux contributeurs**, selon un rythme **aligné sur l'activité réelle** du projet.

**Aucune émission programmatique automatique** : chaque attribution est décidée et publiée. Le rythme reste flexible pour s'adapter à la croissance organique de l'écosystème.

**Transparence** : rapport trimestriel public listant chaque attribution avec sa justification.

**Catégories d'éligibilité** :
- Contributions techniques (code, bug bounty, outils)
- Documentation et traduction
- Support communautaire
- Outils tiers utiles à l'écosystème (wallets, explorers, intégrations)

---

## 5. Cycle Économique & Frais

Chaque transaction réseau applique des frais de **0,05%** avec un **plancher minimum de 0,0001 VinX**.

### Répartition des frais

- **80% au Pool de Staking** : redistribués aux utilisateurs qui bloquent leurs VinX pour stabiliser l'économie
- **20% à la Treasury VinX Labs** : couvre les coûts d'infrastructure des validateurs et le développement

```
[Sandbox 21M] ──► [Contributeurs] ──► [Circulation P2P]
                                              │
   ┌──────────────────────────────────────────┘
   ▼
[Frais de Transaction 0,05%]
   ├── 80% ──► [Pool Staking] ──► (Rendement passif holders)
   └── 20% ──► [Treasury VinX Labs] ──► (Infrastructure et dev)
```

Ce cycle continue **perpétuellement**, garantissant la pérennité économique du protocole.

### Staking

- Stake minimum : 1 VinX
- Warm-up : 7 jours
- Pas de lock, déstaking instantané
- Pas de slashing
- Distribution toutes les 100 blocs (~17 minutes)

---

## 6. Infrastructure : Validateurs & Full Nodes

### 6.1 Validateurs Core (FBA)

Liste restreinte de nœuds sélectionnés et opérés par VinX Labs et des partenaires de confiance.

**Rôle** : signer techniquement les blocs, maintenir le consensus, protéger contre les attaques (notamment l'attaque des 51%).

**Évolution** : 1 (dev) → 5 (lancement) → 9 (stabilisation) → 21 (maturité). Les validateurs ne sont **pas rémunérés directement** — leur incitation est la légitimité, l'influence et l'utilité stratégique.

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

En Phase 1, la gouvernance est assurée par **VinX Labs** via un compte Admin **multi-signature 3/5** (4/5 pour les opérations critiques).

Trois comptes admin spécialisés :
- **Treasury** : reçoit les 20% des frais
- **Operations** : finance le développement et l'infrastructure
- **Admin** : exécute les actions techniques privilégiées (gel judiciaire, upgrades)

### Forkless Upgrades

Grâce à Substrate, les mises à jour majeures du protocole se font **directement on-chain**, sans hard fork :

- **Patch** : 7 jours d'annonce
- **Minor** : 30 jours
- **Major** : 90 jours

Le protocole peut évoluer sans jamais diviser sa communauté.

---

## 8. Règles Immuables

Les éléments suivants **ne peuvent jamais être modifiés**, par aucun mécanisme :

1. **Cap de 100 milliards** de VinX (jamais augmenté)
2. **Architecture des deux compartiments** (Sandbox 21M + Coffre Maturité 99,979 Md)
3. **Consensus FBA** (pas de switch vers PoS ou PoW)
4. **Aucun burn** (jamais)
5. **Propriété des comptes** (saisie possible uniquement sur décision judiciaire, gel temporaire avec unfreeze automatique à 12 mois)
6. **Conditions de déverrouillage du Coffre Maturité** (3 conditions cumulatives strictes)
7. **Répartition des frais 80/20** stakers/Treasury

Ces règles sont protégées techniquement par des assertions runtime et un pallet dédié qui les vérifie à chaque bloc.

---

## 9. Conformité

VinX Ledger cible le marché européen sous le cadre **MiCA**.

- **Pas de portail commercial en Phase 1** : pas de statut CASP requis
- **Aucun KYC obligatoire au protocole** : la transparence des transactions on-chain suffit aux exigences AML
- **Gel sur décision judiciaire uniquement**, unfreeze automatique à 12 mois
- **Comptes inactifs intouchables** perpétuellement

Lorsque le Coffre Maturité sera déverrouillé, VinX Labs aura obtenu son statut CASP MiCA.

---

## 10. Roadmap

Sans calendrier engagé, par phases :

- **Phase 1** : MVP et Sandbox active (21M VinX en circulation progressive)
- **Phase 2** : Testnet public étendu, audit, communauté minimale
- **Phase 3** : Mainnet et déverrouillage progressif du Coffre Maturité (sous conditions strictes)

VinX Ledger n'a pas de pression d'agenda. Le projet avance à son rythme.

---

## Annexe : Synthèse en une page

| Catégorie | Valeur |
|-----------|--------|
| Type | L1 indépendante, non-EVM, account-based |
| Stack | Rust + Substrate |
| Cryptographie | Ed25519, Blake2, Bech32 (`vinx1`) |
| Bloc | 10 secondes |
| TPS Phase 1 | 1 500 |
| Consensus | FBA via Aura + GRANDPA, seuil 80% |
| Validateurs | 1→5→9→21, sélectionnés VinX Labs, non rémunérés |
| Full nodes | Ouverts à tous, sans rémunération en Phase 1 |
| Supply totale | 100 milliards VinX (immuable) |
| Sandbox active | 21 millions VinX (Phase 1) |
| Coffre Maturité | 99,979 milliards VinX (verrouillés, 3 conditions) |
| Distribution Sandbox | Manuelle, par contribution, rythme adapté |
| Frais | 0,05% + floor 0,0001 VinX |
| Répartition frais | 80% stakers / 20% Treasury |
| Staking | 1 VinX min, warm-up 7j, pas de slash |
| Burn | Aucun (jamais) |
| Gouvernance | Centralisée VinX Labs, multi-sig 3/5 (4/5 critique) |
| Upgrades | Forkless via Substrate, SemVer 7/30/90j |
| Conformité | MiCA-compatible, pas de CASP en Phase 1 |

---

*VinX Labs, juin 2026 — Document de référence v2.2*
