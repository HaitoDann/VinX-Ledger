# ADR 0041 — Répartition de l'émission entre subnets par l'usage (melt)

- **Statut :** Proposé (design à décider)
- **Catégorie :** Tokenomics · Modules · **Priorité :** 🔴 haute
- **Date :** Août 2026
- **Lié :** émission élastique (ADR 0040, fournit le total à répartir) ; infrastructure de
  subnets (ADR 0039, consomme le résultat dans les reward pools) ; bond de module (ADR 0010) ;
  adjudication de fraude (ADR 0023) ; invariant de masse (ADR 0004).
- **Amendé par :** ADR 0044 (canal d'émission unique = melt ; « valeur = ce qui est payé, jamais
  jugé » ; deux rails ; CAP + bond proportionnel à l'émission captée) — **à lire avec cet ADR**.

---

## 1. Contexte

L'ADR 0040 fixe **combien** on émet par époque (`E = r·F`). Il reste à décider **vers quels
subnets** cette émission coule, sans (a) comité qui choisit les gagnants, (b) tokens de subnet +
AMM (spéculation), ni (c) scoring subjectif on-chain gameable. C'est le point qui a fait tourner
en rond toute la conception ; deux candidats avaient été posés :

- **Staking à la TAO** (staker du VINX dans un subnet → il touche plus d'émission) → **rejeté** :
  réintroduit spéculation + plutocratie + rendement de staking (contraire à « aucun rendement »
  et « zéro spéculation »). L'émission irait où les riches parient, pas où le réseau sert.
- **Usage / melt** (émission ∝ VINX consommés en échange d'un service) → **retenu** : piloté par
  la **demande réelle**, objectif, sans comité ni spéculation.

## 2. Décision

> **L'émission d'une époque se répartit entre subnets au prorata du VINX melté (consommé pour un
> service) dans chacun sur la fenêtre, sous un plafond par subnet et une porte de bond.**

Le melt joue ses **deux rôles** (cf. 0040) : il remplit la Fonderie (niveau global d'émission)
**et** il pondère ici le partage inter-subnet. Un melt est un flux `circulation → Fonderie`
(recyclage, invariant 0004 préservé), enregistré par subnet.

### 2.1 Poids d'un subnet

Pour le subnet `i`, sur la fenêtre de répartition :

```
poids_i = min( melt_i , CAP · Σ_j melt_j )        si bond_i ≥ MIN_SUBNET_BOND, sinon 0
part_i  = poids_i / Σ_k poids_k
emission_i = part_i · E_fenêtre
```

- **`melt_i`** : total VINX melté en payant des services du subnet `i` sur la fenêtre.
- **Porte de bond** : un subnet dont le bond est sous `MIN_SUBNET_BOND` ne pèse pas — l'entrée
  reste anti-Sybil (réutilise le bond de l'ADR 0010).
- **Plafond `CAP`** : aucun subnet ne peut capter plus de `CAP` (ex. 25 %) de l'émission d'une
  fenêtre, quel que soit son melt — borne l'auto-dealing et la centralisation. **Invariant gravé
  (ADR 0044) : `CAP · k < 1`** (avec `k` le multiplicateur d'amorçage de 0044) → l'auto-dealing est
  une perte sèche. Avec `CAP = 25 %`, cela impose `k < 4`.

`emission_i` **abonde le reward pool** du subnet `i` (ADR 0039), en plus des dépôts clients
(`Deposit`). La répartition **intra**-subnet (quel mineur touche quoi) reste gérée par le
`reward_root` de l'opérateur, adossé à son bond (fraude → slash, ADR 0023).

### 2.2 Ce que la L1 mesure vs ne juge pas

- **Mesure** (objectif, on-chain) : le melt par subnet — un flux de VINX vérifiable.
- **Ne juge pas** : la *qualité* du travail. Le melt mesure la **demande cliente**, pas la sortie
  des mineurs. La distribution interne (subjective pour un travail subjectif) reste l'affaire du
  subnet, garantie par le bond + 0023. → **Tout subnet, même à travail subjectif, peut donc
  toucher de l'émission proportionnelle à son usage réel** (amélioration vs « seuls les subnets
  objectivement vérifiables » : le melt fournit le signal objectif manquant au niveau inter-subnet).

## 3. Résistance Sybil — la condition honnête

Wash-melter chez soi pour capter plus d'émission coûte le melt (flux réel vers la Fonderie) et
ne rapporte qu'une part de `E`. **C'est non rentable tant que le melt réel total ≫ l'émission
`E`.** Le point faible est l'**amorçage** (usage minuscule → gonfler son subnet est bon marché).
Trois garde-fous, actifs surtout tôt :

1. **Plafond `CAP`** par subnet et par fenêtre (borne le gain d'un wash).
2. **Porte de bond** `MIN_SUBNET_BOND` (l'entrée a un coût immobilisé).
3. **Phase d'amorçage** (à trancher, §5) : partage égal entre subnets bondés vérifiés au départ,
   puis bascule vers le poids-melt quand le volume réel dépasse un seuil.

Le dommage max d'un opérateur malhonnête reste **borné** : il ne peut capter au plus que `CAP`
de `E`, et il a dû **melter pour de vrai** (coût irrécupérable) pour l'obtenir.

## 4. Conséquences

**Positif**
- Émission dirigée par la **demande réelle**, sans comité, sans token de subnet, sans
  spéculation. **Boucle fermée non-spéculative** : dépenser pour un service *est* ce qui
  distribue l'émission aux fournisseurs de ce service.
- **Tout type de subnet** peut être financé par émission (le melt objective l'inter-subnet ; le
  bond+0023 garde honnête l'intra-subnet).
- Réutilise bond (0010), reward pool (0039), invariant (0004).

**Coûts / pièges**
- **Wash-farming à l'amorçage** (mitigé : plafond + gate-bond + phase égale).
- Réintroduit le **melt** (retiré auparavant) — flux `circulation → Fonderie` à implémenter avec
  soin (invariant 0004).
- Fenêtre de répartition = surface de mesure supplémentaire dans l'état (bornée : un compteur de
  melt par subnet, remis à zéro par fenêtre).

## 5. Questions ouvertes

1. **Amorçage** : partage égal (subnets bondés vérifiés) puis bascule au seuil, **ou** poids-melt
   dès le départ avec plafond + gate-bond ? (Recommandé : partage égal court, puis bascule.)
2. **Valeurs** : `CAP` (ex. 25 %), `MIN_SUBNET_BOND`, longueur de la fenêtre de répartition — à
   graver comme constantes testées (tripwire).
3. **Signal d'usage** : melt (coûteux, fort anti-Sybil — retenu) vs simple throughput mesuré
   (moins cher à fausser — écarté).
4. **Mécanique de consommation** : comment un client « melt » en payant un service (portion de
   l'`Deposit` reversée à la Fonderie ? opération dédiée ?) — à spécifier avec 0039.

## 6. Alternatives écartées

- **Staking à la TAO / dTAO.** Rejeté (§1) : spéculation + plutocratie + rendement de staking.
- **Répartition par un comité de gouvernance.** Rejeté : tue la neutralité du fair launch.
- **Émission égale par subnet, en permanence.** Rejeté : Sybil trivial (enregistrer N subnets
  vides) et déconnecté de l'usage. (Retenue seulement comme *phase d'amorçage* bornée, §5.)
- **Throughput au lieu du melt.** Rejeté : un paiement récupérable est bien moins coûteux à
  wash qu'un melt irréversible.
