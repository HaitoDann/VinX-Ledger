# ADR 0033 — Genèse & bootstrap de fair-launch

- **Statut :** Partiellement supersédé — **§1 (genèse multi-validateurs + genesis_hash) reste valide** ;
  §2 supersédé par ADR 0038 (Open PoS) ; §3 adressé par ADR 0040 (émission progressive, `T_half` allongé).
- **Catégorie :** Consensus & finalité / Tokenomics · **Priorité :** 🟠 moyenne
- **Date :** Juillet 2026 — révisé Août 2026
- **Liens :** concrétise l'esprit fair-launch (whitepaper, ADR 0021) ; §2 → ADR 0038 ;
  §3 → ADR 0040 ; s'articule avec ADR 0028 (partage d'émission par époque).

> **Note de révision (août 2026)** : les deux problèmes principaux de cet ADR ont été
> résolus séparément et plus précisément :
> - **§2 (admission permissionless)** → ADR 0038 (Open PoS) définit la mécanique complète
>   (bond → pool, warmup 3 époques, rotation époque top-N, bornes immuables).
> - **§3 (lissage émission early)** → ADR 0040 allonge `T_half` à ~20 ans, ce qui réduit
>   structurellement le front-loading sans toucher la courbe elle-même.
> - **§1 (genèse multi-validateurs + genesis_hash)** reste à implémenter — c'est la
>   partie la plus concrète et la seule encore ouverte de cet ADR.

## Contexte

VinX se veut un **fair launch** : pas de premine, toute la masse dans la Fonderie au bloc 0,
émise uniquement par le travail (ADR 0021). Mais **le lancement lui-même n'est pas cadré**. Le
code a un `GenesisConfig { chain_id, admin_address, validator_address }` — donc en pratique la
genèse démarre avec **un** validateur et **une** clé admin. Or c'est précisément la fenêtre où
tout se joue :

- l'émission est **front-loadée** (50 % de la masse sur 8 ans, halving) ;
- au repos, le premier bloc produit après une période d'activité **forge toute l'émission
  accumulée** à son producteur (cf. simulation) ;
- si le set de départ est **petit et fermé**, une poignée d'acteurs capte l'émission la plus
  riche du réseau → contredit le fair-launch et concentre le pouvoir.

Le risque n'est donc pas « les early gagnent beaucoup » (c'est le mécanisme de distribution
d'un fair-launch, et c'est assumé) — c'est **« PEU gagnent beaucoup parce que le set est
petit et gated pendant la fenêtre de forte émission »**. Le remède se joue **à la genèse et
dans les premières semaines**.

## Décision proposée

Définir un **protocole de lancement** en trois volets : la genèse elle-même, l'ouverture du
set, et un lissage optionnel de l'émission early.

### 1. Genèse multi-validateurs & paramètres explicites

- `GenesisConfig` étendu à un **set initial** (`Vec<Address>`) plutôt qu'un validateur unique,
  chacun avec son bond (ou grandfathered au bootstrap), et un **comité admin K-of-M** initial
  (ADR 0011) plutôt qu'une clé unique.
- **Tous les paramètres consensus/éco** figés à la genèse et **empreintés dans le hash de
  genèse** (chain_id, set initial, comité, fee_floor, versions) → la genèse est un engagement
  vérifiable, pas une config muable.
- Un `genesis_hash` canonique (vecteur doré, ADR 0020) que tout nœud recompute et compare —
  deux nœuds avec des genèses différentes ne peuvent pas se parler (anti-split réseau).

### 2. Ouverture permissionless précoce du set (le levier clé)

C'est **le** remède au Q4 : rendre l'admission de validateur **permissionless par bond dès le
départ** (poster le bond suffit à rejoindre la rotation), au lieu de la gouvernance-gated
actuelle. Ainsi, **le marché concourt pour les récompenses early les plus riches** → le set
grossit *précisément quand l'émission est la plus forte* → la distribution s'élargit
naturellement au lieu d'être captée.

- La résistance Sybil repose sur le **bond** (skin-in-the-game), pas sur une whitelist.
- Combiné à l'**émission non pondérée par le bond** (déjà le cas) : pas d'avantage aux gros.
- Combiné au **partage d'émission sur le quorum** (ADR 0028) : chaque nouveau validateur qui
  co-signe est rémunéré → incitation à rejoindre tôt.

### 3. (Optionnel, à débattre) lissage de l'émission early

Ne **pas** toucher la courbe (0021 immuable). Mais deux pistes *périphériques* possibles, à
peser :
- **Plafond de forge par bloc** : borner l'émission qu'un seul bloc peut forger (l'excédent
  reste dû et se forge aux blocs suivants) → tue le grumeau et l'incitation à retarder, sans
  changer la masse totale.
- **Rampe d'ouverture** : ne pas exiger le bond plein les premières semaines (le réseau n'a
  pas encore de valeur → 100 k VINX est un ticket cher au bootstrap) — un bond progressif
  abaisse la barrière d'entrée early, quand la décentralisation compte le plus.

## Conséquences

**Positif**
- Le fair-launch devient **réel** (distribution large) et non juste **déclaratif**.
- Un `genesis_hash` empreinté durcit le réseau contre les splits de configuration.
- L'ouverture permissionless transforme le front-loading d'un **risque de capture** en un
  **aimant à décentralisation**.

**Coûts / pièges**
- L'ouverture permissionless change le modèle de sécurité (set dynamique, Sybil par bond) et
  **dépend d'un consensus multi-validateur éprouvé** (banc n≥3, ADR 0002/0027/0031) — à ne pas
  activer avant.
- Le plafond de forge et la rampe de bond frôlent la politique monétaire : à cadrer pour ne
  **pas** empiéter sur l'immuabilité de l'émission (0021) — ce sont des règles de *distribution/
  d'admission*, pas de *masse*.
- La genèse multi-validateurs exige une **cérémonie** hors-chaîne (qui sont les N initiaux, qui
  détient le comité) — un point de confiance résiduel au tout début, à documenter honnêtement.

## Alternatives écartées

- **Bootstrap mono-validateur puis ouverture “plus tard”** (statu quo implicite) : risqué —
  “plus tard” arrive souvent après que l'émission riche est déjà captée.
- **Premine/allocation pour amorcer** : rejeté — contredit frontalement le fair-launch.
- **Toucher la courbe d'émission pour aplatir le début** : rejeté — 0021 l'a gravée immuable
  (feature de confiance). On agit sur *distribution/admission*, jamais sur la *masse*.

## Notes d'implémentation

- `vinx-state/genesis` : `GenesisConfig` multi-validateurs + comité K-of-M ; `genesis_hash`
  canonique (vecteur doré).
- `vinx-state` : bascule admission validateur gouvernance-gated → **permissionless par bond**
  (nouvelle voie, ou action `AddValidator` sans check admin quand le bond est posté) — derrière
  le prérequis consensus multi-validateur.
- (si retenu) plafond de forge par bloc dans `emit_work_reward` ; rampe de bond paramétrée par
  hauteur/temps depuis la genèse.
- Dépendances : ADR 0002/0027/0031 (consensus n≥3 éprouvé) **avant** l'ouverture ; ADR 0028
  (partage d'émission) en synergie ; ADR 0021 (ne pas violer l'immuabilité).
