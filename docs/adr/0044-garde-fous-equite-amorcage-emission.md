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

> **Invariant gravé : `CAP · k < 1`.** C'est la condition de sûreté qui rend l'auto-dealing
> **structurellement non rentable**.

- **Preuve.** Un acteur qui melte `M` pour tirer l'émission vers son propre subnet reçoit au plus
  `CAP · E_fenêtre = CAP · k · M` (plafond par subnet appliqué sur l'émission de la fenêtre, elle-
  même plafonnée à `k·M` par §2.1). Si `CAP · k < 1`, il **récupère moins que `M`** → **perte
  sèche**. « Melter tout pour tout récupérer via l'émission » est donc impossible à rentabiliser.
  En pratique il récupère encore moins (l'émission est partagée avec les vrais subnets).
- **Multiplier les subnets ne contourne pas** : chaque subnet reste plafonné à `CAP` et perd
  (`CAP·k<1`), et chacun exige son propre bond → l'attaque coûte plus cher à mesure qu'on la
  duplique. Avec `CAP = 25 %`, l'invariant impose **`k < 4`**.
- **Le burn (melt) est le coût anti-Sybil, pas du gaspillage.** Mesurer la demande *sans* brûler
  (simple volume de paiement) serait **wash-tradeable** (s'auto-payer en boucle, gratuitement).
  Le burn rend coûteux de **simuler** de la demande — c'est le cœur du mécanisme.
- **CAP par subnet** (ex. 25 %) : le surplus non capté **retourne à la Fonderie**.
- **Bond proportionnel à l'émission captée** : capter une grosse part exige un bond conséquent au
  risque (défense complémentaire à `CAP·k<1`).

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

En **fee-only** (§2.7, Q1), les validateurs ne frappent rien → une chaîne partie de zéro est en
**deadlock** (pas de token → pas de tx → pas de frais → rien n'existe). **Une circulation
initiale au genesis est donc obligatoire** ; la formule « zéro jeton à la genèse » est
**impossible** et abandonnée.

> **Amorçage par le travail (« subnet 0 »).** Plutôt qu'une *allocation*, le **seed float** est
> **gagné** : une phase de bootstrap **plafonnée et temporaire** émet le seed contre un **travail
> objectif et utile** (ex. stockage : Go prouvés — lance un vrai subnet dès le jour 1), en
> **supply-push assumé** (la seule fenêtre sanctionnée, bornée par le plafond), puis **bascule en
> demand-pull**. C'est le modèle Bitcoin/Bittensor (« pas d'allocation, tout gagné »), plus
> aligné que l'allocation. Garde-fous : plafond = taille du seed, **plafond par participant**,
> bond anti-Sybil, transition nette. Le bootstrap joue **double rôle : amorçage *et* filet de
> distribution** si l'écosystème subnet tarde. Paramètres (taille du seed, service, durée) →
> **ADR 0033**. Une **petite trésorerie Labs vestée** (0033) finance le dev à côté (le seed
> gagné va aux travailleurs, pas à Labs).

### 2.7 Modèle d'émission retenu (Q1) — *cadre l'ensemble*

> **Décision : l'émission se fait par les subnets (demand-pull / melt) ; les validateurs vivent
> des frais (fee-only PoA).** C'est le modèle le mieux aligné — quasiment l'**unique** satisfaisant
> toutes les contraintes (cap dur · pas de plutocratie · fair/large · demande réelle · sans oracle
> · anti-Sybil · onboarding permissionless).

- **Rejet des alternatives :** PoW (gaspillage + capture matérielle), PoS/émission-aux-stakers
  (**plutocratie**), émission-aux-validateurs-PoA (concentration au set curé), usage-mining
  générique (Sybil/farming), style Bittensor **sans** melt = **stake + notation Yuma** (plutocratie
  + oracle — exactement ce que le melt évite).
- **Le melt est porteur de 3 rôles** qu'aucune alternative ne cumule : (1) **dénominateur commun**
  (comparer 1 Go vs 1 h-GPU vs 1 annotation en « VINX payés », sans oracle) ; (2) **résistance
  Sybil** (fausser la demande brûle de vrais tokens) ; (3) **onboarding permissionless** (un
  nouveau subnet touche sa part par son melt, sans comité).
- **Deux couches (Q1) :** **sécurité = PoS permissionless** (comité VRF, admission par bond, ADR 0029/0038) ;
  **économie = permissionless** (n'importe qui lance un subnet ou mine). La décentralisation est là
  où elle compte : *qui peut gagner/participer = tout le monde*.
- **Le pari assumé :** la distribution des ~90 % de la Fonderie **dépend du succès des subnets**
  (pas d'usage → émission inerte). Mitigations : ≥ 1 subnet réel **au lancement**, et le bootstrap
  §2.6 comme **filet**.

## 3. Conséquences

- **Équité de lancement** : impossible pour 1-2 mineurs de rafler l'émission — pas d'usage réel,
  pas d'émission (elle attend dans la Fonderie).
- **Simplicité** : un seul canal (melt), aucun oracle, aucun comité, aucun scoring on-chain.
- **À implémenter** (avec 0040/0041/0042) : accumulateur de melt par subnet et par fenêtre ;
  plafond `min(r·F·Δt, k·M)` au règlement d'époque ; bond proportionnel ; comptabilité des deux
  rails. Paramètres à caler par simulation (`scripts/emission_sim.py`) : `k`, `CAP`, bond.
- **Négatif / vigilance** : `k` mal calé (trop haut → jackpot ; trop bas → pas d'incitation) ;
  dépendance externe du pont §2.5 (volatilité/régulation) à diversifier.
