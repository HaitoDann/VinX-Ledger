# VinX Ledger

**Version :** 0.1 (Draft)
**Date :** 21 mai 2026
**Éditeur :** VinX Labs
**Statut :** Document de cadrage pré-développement

---

## 1. Vision et positionnement

VinX Ledger est une blockchain de paiement conçue pour l'**usage quotidien**. Elle vise à offrir une monnaie numérique simple, peu coûteuse, rapide et résistante à l'inflation, exploitable aussi bien pour des paiements entre particuliers que pour des paiements marchands.

VinX n'est ni un store of value spéculatif, ni un stablecoin adossé à une devise fiat. C'est une **monnaie autonome** avec son propre cours, dont la stabilité dans le temps est assurée par une **supply fixe** et l'absence de mécanismes inflationnistes.

### Principes fondateurs

- **Simplicité d'usage** : précision affichée à 2 décimales, comme une monnaie classique.
- **Coût d'usage minime** : frais de transaction de 0.05% avec un floor minimum ajustable.
- **Résistance à l'inflation** : supply fixe de 100 milliards de VinX, aucune émission ne pourra jamais dépasser ce plafond.
- **Évolutivité technique** : architecture pensée pour monter en charge sans rupture protocolaire.
- **Gouvernance assumée** : VinX Labs opère et fait évoluer le protocole, avec un socle de règles immuables protégeant les utilisateurs.

---

## 2. Architecture technique

### 2.1 Type de réseau

- **Blockchain L1 propre** (pas un L2, pas un token sur une chaîne existante).
- **Non-EVM** : machine d'exécution propriétaire optimisée pour les paiements.
- **Réseau public** : n'importe qui peut créer un compte, émettre des transactions et consulter la chaîne. Le KYC est délégué aux on/off-ramps (portail de vente, exchanges).

### 2.2 Consensus

VinX Ledger utilise un consensus **Federated Byzantine Agreement (FBA)** inspiré de XRP (RPCA) et Stellar (SCP).

- **Validateurs sélectionnés par VinX Labs**, non rémunérés.
- **Seuil de validation** : 80% d'accord entre validateurs sur une UNL (Unique Node List) officielle.
- **Pas de Proof-of-Work, pas de Proof-of-Stake** au niveau du consensus.

#### Phases de déploiement des validateurs

| Phase | Période | Nombre de validateurs | Opérateur |
|-------|---------|----------------------|-----------|
| Testnet / Dev | Phase de développement | 1 | VinX Labs |
| Mainnet — lancement | Année 0-1 | 5 | VinX Labs |
| Mainnet — stabilisation | Année 1-2 | 9 | VinX Labs |
| Mainnet — cible long terme | Année 2+ | 21 | VinX Labs + partenaires de confiance |

#### Justification du chiffre de 21 validateurs à terme

- **Sécurité BFT** : 21 validateurs tolèrent 4 défaillances simultanées (~19%), bien en dessous du seuil byzantin de 33%.
- **Performance** : au-delà de 30 validateurs, la latence de consensus croît significativement (communication quadratique). 21 reste compatible avec le bloc cible de 10 secondes.
- **Décentralisation crédible** : 21 places suffisent pour inviter banques, exchanges, universités, ONG, États partenaires, sans saturer le réseau.
- **Cohérence avec l'écosystème** : Stellar tier 1 ~23 validateurs, XRP UNL ~35.

### 2.3 Paramètres de blocs

| Paramètre | Valeur |
|-----------|--------|
| Temps de bloc cible | 10 secondes |
| Finalité | Déterministe (à la validation du bloc) |
| Throughput phase 1 | 1 500 TPS |
| Throughput cible moyen terme | 3 000-5 000 TPS (optimisations protocolaires) |
| Throughput cible long terme | 50 000+ TPS (couches de paiement off-chain) |

### 2.4 Modèle de comptes

- **Account-based** (et non UTXO).
- Chaque compte est identifié par une paire de clés **Ed25519** (signatures rapides, courtes, standard moderne).
- Pas de réserve minimum requise pour exister.
- Adresse dérivée d'un hash de la clé publique.

### 2.5 Format d'adresse

Format **Bech32**, préfixe `vinx1`.

- Exemple : `vinx1q9a7k3m2x8r5w7v4n6p2t9c3l5y8j6h4f2d1s`
- Avantages : lisibilité humaine, détection automatique des erreurs de frappe (checksum intégré), standard moderne déjà adopté par Bitcoin SegWit, Cosmos, Polkadot.

### 2.6 Confidentialité

- **Transparence par défaut** : toutes les transactions sont publiques et consultables sur l'explorer officiel.
- Cohérent avec la conformité MiCA et les obligations AML.
- Une couche de confidentialité optionnelle (stealth addresses, ZK) pourra être étudiée en phase ultérieure si le besoin émerge.

### 2.7 Stratégie d'évolutivité

L'évolutivité est traitée par couches successives, sans complexifier le L1 :

1. **Court terme (post-lancement)** : optimisations protocolaires — parallélisation des signatures, batching des transactions, compression. Objectif 3 000-5 000 TPS sans changer la structure.
2. **Moyen terme** : **canaux de paiement** (modèle inspiré de Lightning) pour les paiements récurrents marchands. Les petites transactions se font off-chain, settlement on-chain périodique. Permet d'atteindre 50 000+ TPS effectifs.
3. **Long terme (si besoin uniquement)** : sharding optionnel. À ne pas construire prématurément.

Le L1 reste volontairement simple et fiable. Les couches d'optimisation viennent au-dessus.

### 2.8 Primitives supportées (phase 1)

Phase 1 : **pure monnaie**. Pas de Turing-complétude.

- Paiement simple (compte A → compte B)
- Multi-signature (M-of-N pour comptes partagés, entreprises, gouvernance)
- Escrow basique (paiement conditionnel libérable après délai ou condition simple)

Évolutions futures (smart contracts, DEX, NFT) à envisager selon l'adoption.

---

## 3. Précision et unités

| Élément | Valeur |
|---------|--------|
| Décimales internes | 18 |
| Décimales affichées à l'utilisateur | 2 |
| Symbole de devise | VinX |
| Ticker | VINX |
| Nom des sous-unités | À définir ultérieurement |

Les 16 décimales non affichées sont utilisées pour :

- Micro-paiements (contenus, IoT, streaming)
- Calcul précis des frais (% sur petits montants)
- Splits automatiques des récompenses de staking
- Calculs internes du protocole

---

## 4. Supply et émission

### 4.1 Supply

- **Cap maximum absolu : 100 000 000 000 VinX (100 milliards).**
- Cap **immuable** : aucune modification ne pourra jamais augmenter ce plafond.
- **Pas de burn** : aucun mécanisme ne pourra jamais détruire de jetons.

### 4.2 Allocation au génesis

| Allocation | Quantité | % du cap | Disponibilité |
|------------|----------|----------|---------------|
| Compte admin VinX Labs | 500 000 000 VinX | 0.5% | Immédiate au génesis |
| Réserve protocolaire | 99 500 000 000 VinX | 99.5% | Émission linéaire sur 10 ans |
| **Total** | **100 000 000 000 VinX** | **100%** | — |

Le compte admin sert à : financer l'infrastructure, couvrir les frais opérationnels, financer le développement, sécuriser les partenariats stratégiques. Aucune allocation fondateur ni équipe, aucun airdrop.

### 4.3 Mécanisme d'émission

**Émission linéaire bloc par bloc sur 10 ans** depuis la réserve protocolaire vers un pool de vente publique.

- **Total à émettre** : 99.5 milliards de VinX
- **Durée d'émission** : 10 ans
- **Nombre de blocs sur 10 ans** : ~31 536 000 (10 ans × 365.25 jours × 24 h × 60 min × 6 blocs/min)
- **Émission par bloc** : ~3 155 VinX
- **Émission par jour** : ~27 250 000 VinX (~27.25 M)
- **Émission par an** : ~9 950 000 000 VinX (~9.95 Md)

> Note : les chiffres précis seront affinés lors de l'implémentation pour garantir un total exact de 99.5 Md sur la période, en compensant les éventuels écarts d'arrondi sur le dernier bloc.

### 4.4 Distribution publique

- Les VinX libérés à chaque bloc rejoignent un **pool de vente publique** géré par VinX Labs.
- Vente via le **portail officiel** VinX Labs.
- **KYC obligatoire** pour les acheteurs (conformité MiCA / AML).
- **Cap d'achat par compte** sur une fenêtre temporelle donnée (paramètre à finaliser, ex : 0.1% du pool disponible à un instant T) pour empêcher la concentration excessive.
- Les VinX non vendus restent dans le pool et restent disponibles indéfiniment.

### 4.5 Pourquoi ce modèle

- **Pas de halving** → pas de concentration chez les early users alors que la base d'utilisateurs est petite.
- **Pas de pre-mint massif** → impossible pour un acteur d'acheter une part dominante à prix bas.
- **Émission continue et prévisible** → planification claire pour l'écosystème.
- **Cohérent avec l'usage quotidien** → la supply circulante grandit avec l'adoption, sans choc d'offre.

---

## 5. Frais de transaction

### 5.1 Structure

- **Frais en pourcentage** : 0.05% du montant transféré.
- **Floor minimum fixe** en VinX (à définir au lancement, ajustable par VinX Labs si le cours du VinX évolue significativement).
- Les frais sont **payés en VinX** uniquement.
- Pas de token de gas séparé.

### 5.2 Répartition des frais

| Bénéficiaire | Part |
|--------------|------|
| Pool de staking (redistribué aux holders) | 80% |
| Trésorerie VinX Labs | 20% |

### 5.3 Anti-spam

Le floor minimum fixe empêche les attaques par flood de micro-transactions sans coût. Ce floor est révisable par VinX Labs en fonction de l'évolution du cours du VinX, pour garantir un coût d'usage négligeable pour l'utilisateur tout en maintenant un coût d'attaque suffisant.

---

## 6. Staking

### 6.1 Principe

Le staking VinX n'est **pas un produit d'investissement à haut rendement**. C'est un **mécanisme de redistribution des frais de transaction aux holders fidèles**. Plus VinX est utilisé, plus les stakers gagnent.

### 6.2 Paramètres

| Paramètre | Valeur |
|-----------|--------|
| Stake minimum | Très bas (équivalent ~1€ au lancement) |
| Période de lock | Aucune — liquidité préservée |
| Slashing | Aucun |
| Distribution des récompenses | Continue, à intervalle régulier de blocs |
| Source des récompenses | 80% des frais de transaction collectés |

### 6.3 APY attendu

L'APY suit l'usage réel du réseau. Au lancement, l'APY sera très faible — c'est attendu et assumé. Il croîtra avec le volume de transactions.

> **Promesse de communication** : *« Le staking VinX n'est pas une promesse de rendement spéculatif. C'est votre part de l'économie quotidienne du réseau. »*

---

## 7. Gouvernance

### 7.1 Modèle

- Gouvernance **centralisée chez VinX Labs**.
- Pas de DAO, pas de token de gouvernance séparé.
- Décentralisation progressive **non engagée** à ce stade.

### 7.2 Règles immuables

Les éléments suivants ne peuvent **jamais** être modifiés, par aucun mécanisme :

1. **Supply maximale** : 100 milliards de VinX, immuable.
2. **Absence de burn** : aucun mécanisme de destruction de tokens ne pourra être introduit.
3. **Précision** : 18 décimales internes, immuables.

### 7.3 Règles évolutives

Les paramètres suivants peuvent être modifiés par décision de VinX Labs via une procédure de mise à jour du protocole :

- Pourcentage des frais (actuellement 0.05%)
- Floor minimum des frais
- Répartition des frais (actuellement 80/20)
- Liste des validateurs (UNL officielle)
- Paramètres techniques (taille de bloc, throughput, etc.)
- Ajout de nouvelles primitives (smart contracts, DEX, NFT, etc.)
- Paramètres de staking

### 7.4 Mécanisme de mise à jour

Procédure de versioning du protocole avec activation par bloc de référence (hauteur de bloc planifiée) pour permettre aux validateurs de se mettre à jour en amont. Détails techniques à spécifier en phase d'implémentation.

---

## 8. Gel de compte (Frozen Account)

### 8.1 Principe

Pour respecter le cadre légal (MiCA, AMLD6, ordonnances judiciaires) tout en préservant les droits des utilisateurs, VinX Ledger implémente un mécanisme de **gel de compte sur décision judiciaire**.

### 8.2 Procédure

- **Déclencheur unique** : décision de justice formelle (mandat, ordonnance judiciaire, requête d'autorité compétente reconnue).
- **Mécanisme technique** : un flag `frozen: true` sur le compte, posé par une transaction **multi-signature** du compte admin VinX Labs (M-of-N requis pour éviter tout abus unilatéral).
- **Effet** : impossibilité d'émettre des transactions sortantes depuis le compte gelé. Les fonds **restent la propriété** du compte — il s'agit d'un gel, pas d'une saisie.
- **Transparence** : VinX Labs publie un **rapport trimestriel de transparence** listant tous les gels en cours, avec référence à la décision judiciaire concernée.

### 8.3 Déblocage

- Sur nouvelle décision judiciaire de levée du gel.
- **Délai automatique d'unfreeze** : si aucune confirmation judiciaire n'est apportée dans un délai de **12 mois** suivant le gel initial, le compte est automatiquement défreezé. Ceci protège contre les gels abusifs ou oubliés.

### 8.4 Limites assumées

- **Aucune réversion** des transactions individuelles. Une erreur de manipulation (mauvaise adresse, montant erroné) est irréversible. La responsabilité incombe à l'UX du wallet (confirmation forte, carnet d'adresses, validations multiples).
- Le gel **ne permet pas** à VinX Labs de saisir, transférer, ou modifier le solde d'un compte utilisateur. Seul un blocage temporaire est possible.

---

## 9. Conformité réglementaire

### 9.1 Cadre cible

- Marché principal : **Union Européenne** (cadre MiCA).
- Conformité **AML / AMLD6** via le mécanisme de gel sur décision judiciaire.
- KYC au niveau des on/off-ramps (portail de vente, exchanges, fiat gateways), pas au niveau protocolaire.

### 9.2 Classification probable du token

À confirmer avec un cabinet juridique spécialisé, mais positionnement probable comme **utility token** ou **other crypto-asset** sous MiCA — non un e-money token ni un asset-referenced token (pas d'adossement à une devise fiat).

### 9.3 VinX Labs

L'entité opératrice (VinX Labs) devra être enregistrée comme **CASP (Crypto-Asset Service Provider)** sous MiCA pour opérer le portail de vente et les services associés.

---

## 10. Synthèse rapide

| Élément | Valeur |
|---------|--------|
| Type | L1 propre, non-EVM, account-based |
| Consensus | FBA type XRP, 80% seuil, validateurs VinX Labs |
| Validateurs | 1 (dev) → 5 (lancement) → 9 → 21 (cible) |
| Temps de bloc | 10 secondes |
| TPS phase 1 | 1 500 |
| Cryptographie | Ed25519 |
| Format d'adresse | Bech32 `vinx1...` |
| Supply max | 100 000 000 000 VinX (immuable) |
| Allocation génesis | 0.5% admin VinX Labs / 99.5% réserve |
| Émission | Linéaire sur 10 ans (~3 155 VinX/bloc) |
| Décimales | 18 internes / 2 affichées |
| Frais | 0.05% + floor fixe ajustable |
| Répartition frais | 80% stakers / 20% VinX Labs |
| Staking | Sans lock, accessible, APY suit l'usage |
| Burn | Aucun, jamais |
| Confidentialité | Transparent par défaut |
| Gouvernance | Centralisée VinX Labs, règles immuables protégées |
| Gel de compte | Sur décision judiciaire, multi-sig, unfreeze auto 12 mois |
| Conformité | MiCA (UE), KYC aux on/off-ramps |

---

## 11. Prochaines étapes recommandées

### Phase 0 — Cadrage finalisé (semaines suivantes)

- Validation juridique du cadre MiCA avec un cabinet spécialisé.
- Définition précise du floor minimum des frais (en VinX et équivalent fiat de référence).
- Définition du cap d'achat par compte sur le portail de vente.
- Choix du nom des sous-unités.
- Spécification technique détaillée du mécanisme de mise à jour du protocole.

### Phase 1 — Développement (à planifier)

- Implémentation du nœud VinX (langage à choisir — Rust recommandé pour la performance et la sûreté mémoire).
- Implémentation du consensus FBA.
- Implémentation des primitives (paiement, multi-sig, escrow).
- Wallet officiel mobile-first.
- Explorer de chaîne public.
- Portail de vente avec KYC.

### Phase 2 — Testnet

- Lancement testnet public avec 1 validateur VinX Labs.
- Programme de bug bounty.
- Itérations sur le protocole.

### Phase 3 — Mainnet

- Lancement mainnet avec 5 validateurs VinX Labs.
- Activation de la vente publique.
- Activation du staking.

### Phase 4 — Stabilisation et adoption

- Montée à 9 puis 21 validateurs.
- Partenariats marchands.
- Listing exchanges.
- Couches de paiement off-chain.

---

*Document de cadrage — version de travail. Sujet à révision par VinX Labs.*
