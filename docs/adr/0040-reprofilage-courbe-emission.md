# ADR 0040 — Reprofilage de la courbe d'émission

- **Statut :** 🚧 Brouillon de discussion (aucun chiffre gravé — à trancher ensemble)
- **Catégorie :** Tokenomics · **Priorité :** 🟠 moyenne
- **Date :** Août 2026
- **Lié :** immutabilité de l'émission (ADR 0021) ; heartbeat/distribution (ADR 0038) ; partage
  au quorum (ADR 0028) ; genèse & fair launch (ADR 0033) ; invariant de masse (ADR 0004).

> ⚠️ **Fenêtre de décision.** L'ADR 0021 grave la courbe comme **immuable** : personne, pas
> même l'admin, ne peut la changer après lancement. Reprofiler n'est donc possible **qu'avant le
> mainnet**. C'est une décision *constitutionnelle*, à prendre une seule fois. D'où ce brouillon
> avant de graver quoi que ce soit.

---

## 1. Contexte — deux problèmes distincts

### 1.1 Le whitepaper et le code ne décrivent pas la même courbe

- **Whitepaper :** exponentielle **continue** `débit(t) = R₀ · 2^(−t/8 ans)`, `R₀ ≈ 8,66 Md/an`.
  Débit qui décroît en douceur ; démarre haut (8,66), passe à 4,33 à l'année 8.
- **Code réel** (`cumulative_emission_atoms`, `amount.rs`) : **halving discret**. L'ère `e` dure
  8 ans (`HALVING_PERIOD_SECS = 252 460 800`) et émet `ERA0_EMISSION_ATOMS >> e` **linéairement**
  sur l'ère (`ERA0 = MAX/2 = 50 Md`). Donc un débit **plat par ère**, avec des **falaises** :

| Ère | Années | Émis sur l'ère | Débit (plat) | Cumulé en fin d'ère |
|-----|--------|----------------|--------------|---------------------|
| 0 | 0–8 | 50 Md | **6,25 Md/an** | 50 % |
| 1 | 8–16 | 25 Md | 3,125 Md/an | 75 % |
| 2 | 16–24 | 12,5 Md | 1,5625 Md/an | 87,5 % |
| 3 | 24–32 | 6,25 Md | 0,781 Md/an | 93,75 % |
| … | … | … | (halving) | → 100 % |

Les deux intègrent bien à 100 Md et donnent 50 % à l'année 8, mais **la forme diffère** : le
whitepaper front-load *plus* dans l'ère 0 (8,66 → 4,33) là où le code est *plat* à 6,25, et le
code introduit des **discontinuités** (le débit est divisé par deux d'un bloc à l'autre au
passage d'ère). **Premier arbitrage : réconcilier doc ↔ code — laquelle est la vérité ?**

### 1.2 Le front-loading est une arme à double tranchant (fair launch)

C'est le cœur du désaccord sur les chiffres. Le front-loading sert à **amorcer les
validateurs** (les précoces gagnent le plus → incitation à démarrer un nœud tôt). Mais l'ADR
0033 pointe le risque miroir : si le set de départ est **petit ou gated** pendant la fenêtre de
forte émission, **peu d'acteurs captent beaucoup** → concentration → *contredit* le fair launch.

Deux forces opposées, à équilibrer par les chiffres :

- **Plus de front-loading** (décroissance rapide, ère courte) → meilleur amorçage, mais risque
  de concentration si le set est petit tôt.
- **Moins de front-loading** (courbe plate, ère longue) → moins de concentration, mais amorçage
  plus faible et l'éthos « récompenser le travail précoce » s'affaiblit.

> **Note.** Le reprofilage est le *bon* levier pour « inciter la mise en place de validateurs »
> (cf. discussion émission) — **pas** un pool séparé. Il agit sur la *forme* à masse totale et
> plafond (100 Md) **inchangés** ; il ne viole ni 0021 (tant qu'on grave avant lancement) ni
> l'invariant de masse (0004). Il se combine avec 0028 (partage au quorum → dilue la capture
> par tête) et 0033 (ouverture permissionless → élargit le set quand l'émission est riche).

## 2. Ce qui N'EST PAS en jeu

- **Le total (100 Md) et le plafond** : intouchables (0021).
- **L'invariant `circulation + Fonderie = 100 Md`** (0004).
- **La règle « émission au temps réel, distribuée par le heartbeat »** (0038).
- **Le basculement fees-only** quand la Fonderie se vide.

Seule la **forme du débit dans le temps** est en discussion.

## 3. Espace des options (axes de décision — à trancher, rien de gravé)

- **Axe A — Forme intra-courbe.**
  (A1) Garder le halving discret plat-par-ère (code actuel).
  (A2) Exponentielle continue lissée (whitepaper) — supprime les falaises, front-load un peu plus.
  (A3) Front-load plus marqué (décroissance initiale plus raide).
  (A4) **Rampe de démarrage** : débit qui *monte* sur les premiers mois puis décroît — évite de
       verser le pic à un set encore minuscule au tout début (répond directement au risque 0033).
- **Axe B — Période de halving.** 8 ans (actuel) est long (Bitcoin = 4). Plus court = plus
  front-loadé ; plus long = plus plat.
- **Axe C — Fraction de l'ère 0.** Actuellement **50 % du total en 8 ans** — déjà très
  front-loadé. La réduire aplatit la distribution précoce.
- **Axe D — Traiter les falaises.** Les discontinuités de débit créent des à-coups d'incitation
  (et un « avant/après halving ») ; une courbe lisse les supprime.
- **Axe E — Contrainte de déterminisme.** Toute forme retenue **doit** rester en **arithmétique
  entière pure** (pas de flottant — exigence consensus). Une exponentielle « continue » sera en
  pratique approximée par paliers fins entiers. À chiffrer.

## 4. Questions ouvertes (le désaccord à résoudre)

1. **Vérité de référence** : on aligne le code sur le whitepaper (exp lissée) ou l'inverse
   (acter le halving discret et corriger le whitepaper) ?
2. **Quel niveau de front-loading** juge-t-on « juste » ? Critère chiffrable proposé : *quelle
   part max du total un set de `k` validateurs peut-il capter sur les 12/24 premiers mois ?*
   — c'est la métrique qui relie directement la courbe au risque de concentration (0033).
3. **Rampe de démarrage (A4)** : oui/non ? Si oui, durée et forme de la montée.
4. **Période de halving** : garde-t-on 8 ans, ou raccourcit-on ?
5. **Lissage des falaises (A2/D)** : vaut-il la complexité entière supplémentaire (Axe E) ?

## 5. Méthode proposée pour trancher les chiffres

Plutôt que de graver des constantes « au doigt mouillé », **simuler** quelques scénarios de
courbe contre un même modèle d'adoption du set de validateurs (par ex. set qui croît de `k₀` à
`k∞` sur `T` mois) et comparer, pour chacun :

- la **part captée par les early** (métrique de concentration, Q2),
- la **récompense annuelle par validateur** au fil du temps (lisibilité de l'incitation),
- la **sensibilité aux falaises** (à-coups au passage d'ère).

On grave la courbe **après** avoir vu ces courbes côte à côte. Aucune valeur n'est actée dans ce
document tant que ce comparatif n'est pas validé ensemble.

## 6. Conséquences (à compléter une fois les chiffres choisis)

- **+** Aligne enfin doc ↔ code (dette actuelle réelle).
- **+** Permet de calibrer l'amorçage validateur **sans** pool d'émission séparé ni violation de
  0021/0004.
- **−** Décision constitutionnelle irréversible après lancement — d'où l'exigence de simulation
  préalable.
- **=** Masse totale, plafond, invariant de masse et bascule fees-only inchangés.

## 7. Alternatives écartées

- **Changer la courbe après lancement / la rendre gouvernable.** Rejeté : viole l'ADR 0021
  (l'immutabilité *est* l'argument de confiance monétaire). Le reprofilage n'a lieu **qu'avant**
  le mainnet.
- **Plafonner l'accrual par bloc pour « aplatir »** (cf. 0038 alt. écartée) : rend la courbe
  dépendante de l'usage → « courbe gravée » molle. Rejeté.
- **Pool d'émission séparé pour amorcer les validateurs** : redondant — le front-loading de la
  courbe *est* déjà ce levier, sans mécanisme neuf ni dilution de l'égalité entre validateurs.
