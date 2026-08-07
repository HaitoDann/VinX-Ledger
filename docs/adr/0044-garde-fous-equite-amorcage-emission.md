# ADR 0044 — Garde-fous d'équité et amorçage de l'émission

- **Statut :** Proposé (design constitutionnel — à décider avant le mainnet, avec 0040/0041)
- **Catégorie :** Tokenomics · **Priorité :** 🔴 haute
- **Date :** Août 2026
- **Amende :** l'émission élastique (ADR 0040), la répartition par le melt (ADR 0041),
  l'infrastructure de subnets (ADR 0039). S'appuie sur l'invariant de masse (ADR 0004),
  l'époque de règlement (ADR 0042), la genèse fair-launch (ADR 0033).

> Consolide en un seul lieu les décisions d'équité et d'amorçage prises après la conception
> initiale de 0040/0041/0039. Objet **immuable** une fois le mainnet lancé.

---

## 1. Contexte — trois failles du modèle usage/melt initial

1. **Démarrage à froid (le jackpot de lancement).** L'émission `E = r·F` est maximale quand la
   Fonderie est **pleine** (au lancement), alors que l'usage réel `M` est ~nul. `E ≫ M` → le
   premier acteur qui génère un peu de melt (même en se payant lui-même) capte une part
   disproportionnée. *Exemple : un dev crée un subnet avec 1-2 mineurs et rafle l'émission.*
2. **Ambiguïté sur ce qui « émet ».** Sans règle claire, on est tenté d'inventer des styles
   d'émission (PoUW « je brûle du GPU → donne-moi du VINX ») qui exigent un **oracle** pour juger
   un travail que personne n'a payé → gameable, centralisé.
3. **Œuf/poule.** Le melt suppose de **déjà détenir** du VINX ; d'où vient le premier ?

## 2. Décisions

### 2.1 Émission plafonnée par l'usage — *amende 0040*

> **Émission distribuée sur une fenêtre = `min( r·F·Δt , k·M_fenêtre )`.** Le surplus **reste
> dans la Fonderie** (il attend une demande réelle, il n'est ni perdu ni « banké » en backlog).

- Pas d'usage → quasi pas d'émission distribuée → **plus de jackpot à froid** ; les jetons
  attendent dans la Fonderie que la vraie demande arrive.
- **`k`** (multiplicateur de subvention d'amorçage, ex. 2–5×) : les vrais early users touchent un
  peu plus que ce qu'ils meltent (incitation), mais **borné** → non exploitable à l'échelle.
- Cohérent avec 0040 : la Fonderie reste un réservoir hard-cappé ; on ajoute « le débit est
  plafonné par l'usage » à **tout instant**, pas seulement à l'équilibre `E*=M*`. Memoryless
  (chaque fenêtre plafonne indépendamment → pas de rattrapage explosif). L'époque (0042) calcule
  ce `min` à chaque règlement.

### 2.2 Un seul canal d'émission : la demande (melt) — *précise 0041*

> **Il n'existe qu'un seul style d'émission : demand-pull par le melt. La valeur est révélée par
> ce que le client paie — la L1 ne la juge jamais.**

- Tout service (stockage, calcul/PoUW, annotation, contenu…) est un **type de service** qui se
  branche sur ce canal unique ; ce n'est **pas** un style d'émission distinct.
- **Rejet du supply-push** : frapper du VINX pour du travail *offert mais non payé* (PoUW pour
  lui-même) est interdit — il faudrait un oracle, c'est gameable, sans signal de demande.
- Conséquence pratique : VinX ne définit jamais « 100 Go = 2 € ». Il constate que des clients ont
  melté ~2 € pour ce service. **La demande mesure, pas l'offre revendiquée.**

### 2.3 Deux rails de rémunération — *précise 0039/0041*

- **Rail 1 — paiement direct (deposit → mineurs).** Le marché : revenu **prévisible**, neutre en
  circulation, **permanent** (fonctionne Fonderie vide). C'est ce qui rend un service viable.
- **Rail 2 — émission dirigée par le melt.** Subvention d'amorçage **bornée** (§2.1) venant de la
  **Fonderie** (la réserve verrouillée qui se distribue), décroissant à mesure que `F` se vide →
  **transition naturelle vers un pur marché de frais** en fin de course.
- Ce n'est **pas circulaire** : le paiement direct est latéral (VINX existant) ; l'émission vient
  de la réserve (VINX neuf), d'un montant différent (bonifié tôt par `k`, ~0 en fin de vie).
- Rappel : le **melt recycle vers la Fonderie** (il ne détruit pas) → son effet sur le cours est
  **cyclique**, pas déflationniste permanent. Le seul moteur durable du prix est l'**utilité**
  (besoin de VINX pour payer/bonder).

### 2.4 Anti-self-dealing — *renforce 0041*

- **CAP par subnet** (ex. 25 %) conservé (0041) ; le surplus non capté **retourne à la Fonderie**.
- **Bond proportionnel à l'émission captée** : capter une grosse part exige un bond conséquent au
  risque → un acteur seul ne peut pas rafler à bas coût (l'auto-dealing devient cher).

### 2.5 Pont valeur-externe (modèle « Qubic ») — *amende 0039*

Un opérateur de subnet peut monétiser la **capacité oisive** sur un protocole externe (ex. miner
Monero à vide). Deux montages **propres** :
- **Montage 1 (marché + melt) :** revenu externe → **acheter du VINX** au marché → **le melt** →
  tire l'émission. *(Émet du VINX protocolaire, mais concentre l'amorçage → soumis à §2.1/2.4.)*
- **Montage 2 (paiement direct, retenu) :** l'opérateur encaisse le revenu externe (finance
  l'infra) et **paie ses mineurs en VINX qu'il détient** (trésorerie). Le protocole **ne frappe
  rien**.

> **Règle dure :** le travail externe **ne mint jamais** de VINX protocolaire directement (pas
> d'oracle, pas de supply-push). Le VINX entre par le **marché** (achat) ou le **paiement direct**.

### 2.6 Amorçage œuf/poule — *renvoie à 0033*

L'émission ne crée **pas** le premier VINX. Un **seed float** en circulation au genesis
(distribution large, transparente, sans premine d'initiés + **faucet** au lancement, ADR 0033)
amorce le côté demande. L'émission distribue la Fonderie **ensuite**, gated par l'usage (§2.1).
Le pont valeur-externe (§2.5) peut aussi amorcer la demande avant qu'elle soit organique.

## 3. Conséquences

- **Équité de lancement** : impossible pour 1-2 mineurs de rafler l'émission — pas d'usage réel,
  pas d'émission (elle attend dans la Fonderie).
- **Simplicité** : un seul canal (melt), aucun oracle, aucun comité, aucun scoring on-chain.
- **À implémenter** (avec 0040/0041/0042) : accumulateur de melt par subnet et par fenêtre ;
  plafond `min(r·F·Δt, k·M)` au règlement d'époque ; bond proportionnel ; comptabilité des deux
  rails. Paramètres à caler par simulation (`scripts/emission_sim.py`) : `k`, `CAP`, bond.
- **Négatif / vigilance** : `k` mal calé (trop haut → jackpot ; trop bas → pas d'incitation) ;
  dépendance externe du pont §2.5 (volatilité/régulation) à diversifier.
