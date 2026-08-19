# ADR 0029 — Décentralisation à grande échelle : agrégation de signatures & comité dynamique

- **Statut :** Accepté — **Architecture cible du consensus** (non implémenté)
- **Catégorie :** Consensus & finalité · **Priorité :** 🔴 haute
- **Date :** Juillet 2026 · Révisé août 2026 (décision architecture VinX PoS)
- **Liens :** étend la finalité (ADR 0002) ; généralise la rémunération (ADR 0028) et le
  jailing (ADR 0027) ; aligne l'admission PoS permissionless (ADR 0038) ; prérequis des
  Appchains (ADR 0050).

## Contexte

Le consensus actuel — **round-robin + TOUS les validateurs co-signent** chaque bloc — a un
coût **O(N)** par bloc : `N` signatures Ed25519 stockées, `N` vérifications, taille de bloc
croissant avec `N`. C'est parfait pour des **dizaines** de validateurs, mais **plafonne** là :
au-delà, la taille des blocs et le coût de vérification étranglent le réseau. La
décentralisation de masse (centaines → milliers de validateurs) est donc **hors de portée du
consensus actuel** tel quel.

Deux verrous, deux techniques complémentaires :

1. **Taille/coût des signatures** → **agrégation** : `N` signatures deviennent **une seule**
   signature agrégée + un bitmap de participation, vérifiable en O(1)~ (une opération de
   couplage). C'est ce que font Ethereum (BLS12-381) et les chaînes Cosmos modernes.
2. **Nombre de signataires requis** → **échantillonnage de comité** : ne plus exiger que
   *tous* signent, mais un **comité tiré au sort** (VRF) par bloc/époque, assez grand pour
   qu'une majorité honnête tienne avec très haute probabilité, et **rotatif** pour la sécurité.

Ce ADR **étend** le consensus (il ne jette ni le round-robin ni le quorum : il les
généralise), et se déploie en **phases** — la phase 1 seule débloque déjà des centaines de
validateurs.

## Décision proposée (phasée)

### Phase 1 — Agrégation de signatures (BLS)

Remplacer les co-signatures Ed25519 par un schéma **agrégeable** (BLS sur BLS12-381) :

- `BlockSignature { validator, pub_key, sig }` × N → **une** signature agrégée + un **bitmap**
  des validateurs présents. La finalité se vérifie en agrégeant les clés publiques du bitmap et
  une seule vérification de couplage contre `BlockHeader::hash()`.
- **Quorum** inchangé sémantiquement (`popcount(bitmap) ≥ quorum`), mais la preuve tient en
  taille **constante** → blocs bornés indépendamment de `N`.
- **Sécurité crypto** : les clés BLS exigent une **preuve de possession** à l'enregistrement du
  validateur (anti *rogue-key attack*), et une **agrégation canonique déterministe** (ordre par
  index de validateur) — consensus-critique (ADR 0020, vecteurs dorés obligatoires).
- Effet immédiat : le goulot O(N) disparaît → **centaines** de validateurs deviennent viables
  *en gardant tout le monde signataire*.

### Phase 2 — Échantillonnage de comité dynamique (VRF)

Quand on vise des **milliers** de validateurs, même l'agrégation ne suffit pas (collecter N
signatures reste un travail réseau O(N)). On **échantillonne** :

- **Beacon d'aléa** : une graine imprévisible mais vérifiable par époque, dérivée de sorties
  **VRF** des blocs précédents (façon RANDAO/aléa-de-chaîne). La résistance au *grinding* (un
  proposeur qui rejoue pour biaiser le tirage) est le point dur à traiter.
- **Sélection VRF** : à chaque hauteur/époque, chaque validateur évalue une VRF `(clé, graine,
  hauteur)` ; il est dans le **comité** (et éventuellement **leader**) si sa sortie tombe sous
  un seuil calibré pour une taille de comité cible `k ≪ N`. La sortie VRF est **déterministe et
  vérifiable** → sûre pour le consensus, et **imprévisible à l'avance** → tue l'attaque de DoS
  ciblé sur le prochain leader connu (faiblesse du round-robin actuel).
- **Seul le comité** propose + co-signe le bloc ; la finalité est le quorum **du comité**
  (agrégé en BLS, phase 1). `k` est dimensionné pour qu'une majorité honnête tienne avec
  probabilité écrasante, étant donné la distribution globale.
- **Tirage égalitaire** : échantillonnage **uniforme parmi les validateurs bondés** (une
  identité = un ticket), **pas** pondéré par le stake — cohérent avec l'ADR 0028 et l'ethos
  VinX. La résistance Sybil repose entièrement sur le **bond** (le tirage uniforme rend le bond
  d'autant plus critique — cf. Alternatives).

### Intégration avec les autres ADR

- **Rémunération (ADR 0028)** : « co-signataires » = **membres du comité ayant signé** ; le
  partage de l'émission se généralise sans changement conceptuel.
- **Jailing (ADR 0027)** : la participation se mesure **conditionnellement à la sélection** (on
  ne pénalise pas un validateur non tiré au sort) — attribution déterministe via la preuve VRF.
- **Light client / weak subjectivity (ADR 0014)** : un comité tournant + dynamique de type PoS
  accentue le besoin de **checkpoints de weak subjectivity** et de preuves de finalité
  compactes (justement fournies par l'agrégat BLS). À faire **avant** la phase 2.

## Modèle

- Registre de clés **BLS** par validateur (+ preuve de possession vérifiée à l'admission).
- Bloc : `agg_sig` (BLS) + `bitmap` (participants) au lieu de `Vec<BlockSignature>`.
- Phase 2 : `beacon` par époque + preuves VRF `(proof, output)` attachées à la proposition et
  aux co-signatures ; vérification d'appartenance au comité par re-évaluation de la VRF.
- Tout est **déterministe et vérifiable** (VRF, agrégation ordonnée) → compatible consensus.

## Conséquences

**Positif**
- Débloque la décentralisation de **masse** (centaines en phase 1, milliers en phase 2) que le
  consensus actuel plafonne.
- Blocs de **taille bornée** indépendamment de `N` (agrégat + bitmap).
- Le VRF supprime la **prévisibilité** du round-robin → robustesse anti-DoS ciblé (complète
  l'ADR 0022).

**Coûts / pièges (assumés : évolution majeure)**
- **Nouvelle primitive cryptographique** (BLS12-381, couplages) : dépendance lourde, surface
  d'audit, *rogue-key* à mitiger par preuve de possession. Ed25519 reste pour les **signatures
  de transactions** (rien n'oblige à tout migrer).
- **Beacon d'aléa** = le vrai point dur : imprévisibilité **et** résistance au grinding, sans
  oracle externe. Sous-problème à instruire séparément (VRF-chain vs drand).
- **Sécurité probabiliste** (phase 2) : une petite probabilité qu'un comité soit compromis si
  `k` est mal calibré ou la distribution trop concentrée → dimensionnement rigoureux requis.
- **Consensus-critique de bout en bout** : changement de format de bloc, de la vérification de
  finalité, de la rémunération. Nécessite le banc multi-nœuds et une spec formelle. À ne lancer
  qu'après 0002 (finalité), 0027 (jailing) et 0014 (weak subjectivity).

## Alternatives écartées

- **Rester en Ed25519 all-sign** : rejeté pour la mise à l'échelle — plafonne à des dizaines.
- **Échantillonnage / vote pondérés par le stake** (façon PoS classique) : écarté par défaut —
  concentre le pouvoir chez les gros bonds, contraire à l'égalitarisme VinX (émission non
  pondérée, ADR 0028). Le tirage **uniforme** est préféré ; il exige en contrepartie un **bond
  suffisant** comme unique barrière Sybil (à cadrer avec un futur ADR Sybil).
- **Threshold signatures (FROST) au lieu d'agrégation par bitmap** : alternative crédible (une
  signature de seuil unique) mais cérémonie de génération de clé distribuée lourde et set
  dynamique difficile ; l'agrégat BLS + bitmap gère mieux un ensemble qui change.
- **Tout faire d'un coup (phase 1 + 2)** : rejeté — la phase 1 (agrégation) livre déjà une
  grande partie du bénéfice à risque bien moindre ; la phase 2 (comité + beacon) est le morceau
  réellement risqué et doit être isolée.

## Décision architecturale (août 2026)

Ce ADR passe de « future » à **architecture cible** du consensus VinX. Le comité VRF avec
agrégation BLS est la direction choisie pour le réseau principal. Les raisons :

1. Le consensus PoA round-robin plafonnerait le set à ~20 validateurs — incompatible avec
   un réseau ouvert et décentralisé.
2. Le modèle Algorand (VRF + comité réduit + BLS) offre finalité déterministe, résistance
   DoS (leader imprévisible), et passage à l'échelle.
3. BLS est déjà implémenté (ADR 0046, `vinx-crypto/bls.rs`) — la phase 1 est faisable sans
   nouvelle dépendance cryptographique.

**Paramètres cibles :**

| Paramètre | Valeur cible | Justification |
|-----------|-------------|---------------|
| Taille du comité `k` | 100 | O(k²) = 10 000 messages — gérable ; sécurité probabiliste élevée |
| Algorithme VRF | **ECVRF RFC 9381** | Standard IETF, crate Rust disponible (`vrf-rs`) |
| Agrégation | **BLS12-381 via `blst`** | Déjà implémenté, audit Ethereum |
| Sélection | **Uniforme parmi les bondés** | Pas pondéré par stake — égalitarisme VinX |
| Leader | **Validateur avec la sortie VRF la plus faible** | Déterministe, vérifiable |
| Finalité | **BFT : 1 bloc, ≥ 67 % du comité** | Déterministe (pas probabiliste) |

**Prérequis avant implémentation :**
- ADR 0002 (finalité) et ADR 0027 (jailing) éprouvés au banc n=3 ✅ (déjà fait)
- ADR 0038 (admission PoS permissionless) décidé ✅ (déjà accepté)
- Spec formelle du beacon d'aléa (seed de l'époque) à rédiger
