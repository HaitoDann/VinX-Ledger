# ADR 0038 — Admission permissionless au set de validateurs (Open PoA)

- **Statut :** Accepté (décision de design) — **non implémenté** à ce jour.
- **Catégorie :** Consensus & finalité / Tokenomics · **Priorité :** 🟠 moyenne-haute
- **Date :** Août 2026
- **Liens :** concrétise le fair launch (ADR 0021, ADR 0033) ; s'articule avec les récompenses
  par époque (ADR 0028) et les garde-fous de gouvernance (ADR 0032) ; dépend du consensus
  multi-validateur éprouvé (ADR 0002/0027/0031) **avant** l'ouverture des phases 2 et 3.

---

## 1. Contexte

VinX est **PoA** (Proof of Authority) : les validateurs sont une liste restreinte qui produit
et co-signe les blocs. Aujourd'hui, l'admission est **gouvernance-gatée** — l'admin ajoute
chaque validateur individuellement via `GovernanceAction::AddValidator`.

Cette centralisation crée une **contradiction structurelle avec le fair launch** :
l'admin choisit qui valide → l'admin choisit qui gagne l'émission. Un réseau qui se prétend
« sans pre-mine, émission par le travail » mais dont le fondateur sélectionne
individuellement les bénéficiaires de ce travail n'est pas fair — c'est un avantage accordé,
pas un travail récompensé.

La question n'est pas de supprimer le PoA (l'identité et la responsabilité légale des
validateurs restent des atouts), mais d'en retirer la dimension de sélection arbitraire.

## 2. Décision

Remplacer la sélection individuelle par un mécanisme **Open PoA** : le bond suffit à entrer
dans la file de candidature ; la sélection nominative est remplacée par un veto collectif.

### 2.1 File de candidature automatique

Toute adresse ayant posté le bond requis (`MIN_VALIDATOR_BOND_ATOMS`, gouvernable) **entre
automatiquement dans la file de candidature** — sans approbation admin individuelle.

L'admin ne conserve qu'**une** prérogative liée aux validateurs : ajuster le montant
du bond minimum via `GovernanceAction::UpdateMinValidatorBond`. Il ne choisit plus les
individus admis.

### 2.2 Veto collectif (fenêtre 7 jours)

À l'entrée en file, une **fenêtre de 7 jours** (`CANDIDATE_VETO_WINDOW_SECS = 604_800`)
s'ouvre pendant laquelle les validateurs actifs peuvent voter pour rejeter le candidat.

- Si **plus de 66 % des validateurs actifs** votent contre dans la fenêtre → rejet ;
  le bond est rendu intégralement ; le candidat peut re-candidater après un cooldown.
- **Absence de veto** dans la fenêtre → admission automatique à l'expiration.
- Le vote est **on-chain** (transaction signée par le validateur) : pas de décision
  discrétionnaire ni hors-chaîne. L'historique est immuable et auditable.

### 2.3 Score S_perf — mérite, pas capital

L'attribution des slots de production dans le set actif suit un score `S_perf` calculé sur
des métriques **100 % déterministes et on-chain** :

- **Taux de co-signature** : `signatures_participées / slots_attendus` sur les N derniers blocs.
- **Taux de proposition réussie** : blocs proposés acceptés par le quorum / total des slots
  où le validateur était leader désigné.

**Règles fermes de S_perf :**

| Règle | Raison |
|---|---|
| Aucun composant de délégation (pas de W_stake) | Évite la spirale plutocratique DPoS |
| Aucune pondération par le bond | Le bond sécurise ; il ne multiplie pas les gains |
| Aucune métrique de latence / timing réseau | Non déterministe — divergence de consensus |
| Métriques calculées uniquement depuis les blocs finalisés | Identique sur tous les nœuds |

L'égalité round-robin est préservée parmi les validateurs de même score : personne n'achète
un avantage de slot avec un bond supérieur au minimum.

### 2.4 Expansion phasée — automatique et immuable depuis la genèse

L'expansion du set suit trois phases dont les **seuils de déclenchement sont gravés à la
genèse** (vecteur doré, ADR 0020) et **non gouvernables** — personne ne peut les modifier
après déploiement.

| Phase | Set cible | Mode d'admission | Déclencheur (gravé, à calibrer à la genèse) |
|---|---|---|---|
| 1 — Bootstrap | 3–5 validateurs | Gouvernance-gated (admin) | Consensus n≥3 éprouvé en banc |
| 2 — Ouverture | 10–21 validateurs | Open PoA (bond + veto collectif) | Seuil de circulation ou durée depuis la genèse |
| 3 — Échelle | 50–101 validateurs | Open PoA (idem, bond adapté) | Seuil de circulation supérieur ou durée |

> La Phase 1 garde la sélection gouvernance-gated le temps que le consensus multi-validateur
> soit éprouvé (ADR 0002, 0027, 0031). Activer Open PoA avant est risqué : un set dynamique
> non testé sous pression peut casser la liveness.

Les **valeurs concrètes** des seuils (en VINX circulants ou en secondes depuis la genèse)
sont définies lors de la cérémonie de genèse et figées dans le `GenesisConfig` (ADR 0033).

## 3. Modèle de confiance

| Ce que l'Open PoA **garantit** | Ce qu'il **ne garantit pas** |
|---|---|
| N'importe qui avec le bond peut candidater | Que tous les candidats seront admis (veto possible) |
| L'admin ne choisit pas les individus admis | Que le veto collectif ne peut pas être instrumentalisé |
| La sélection est on-chain et auditable | L'identité légale des nouveaux validateurs (≠ PoA classique) |

> La Sybil-résistance repose sur le **bond** (capital immobilisé, slashable). Elle ne repose
> plus sur une liste de confiance — c'est le compromis assumé de l'ouverture.

## 4. Conséquences

**Positif**
- Le fair launch devient **cohérent** : l'émission va au travail, et quiconque peut
  s'exposer à ce travail en postant le bond.
- L'admin est **désintéressé** de la sélection des bénéficiaires de l'émission.
- La transparence on-chain du veto rend les rejections auditables et contestables.
- Synergie directe avec ADR 0028 (récompenses par époque) : plus le set est large,
  plus la distribution est efficace.

**Négatif / compromis assumés**
- Un set dynamique complexifie le consensus (churn, fork-choice, jailing). **Dépend
  impérativement** d'ADR 0002, 0027 et 0031 avant d'être activé.
- Le veto collectif peut être instrumentalisé par un cartel de validateurs existants pour
  bloquer la concurrence. Le seuil de 66 % et les garde-fous de gouvernance (ADR 0032)
  atténuent ce risque — à surveiller à l'usage.
- La Phase 1 reste gouvernance-gated — le fondateur garde une sélection centralisée pendant
  le bootstrap. C'est un point de confiance résiduel à documenter honnêtement.

## 5. Alternatives écartées

- **Statu quo (sélection individuelle permanente)** : rejeté — l'admin reste l'arbitre
  permanent de qui gagne l'émission ; contredit le fair launch.
- **DPoS (délégation avec pondération par stake)** : rejeté — réintroduit l'avantage du
  capital, favorise les baleines, recentralise à terme par la spirale commission/délégation.
- **PoUW ou mining fallback** : rejeté — dépendance à la rentabilité d'un protocole externe,
  complexité injustifiée, contredit « monnaie pure ».
- **Airdrop comme mécanisme d'émission** : rejeté — pas de lien avec le travail de sécurité ;
  gameable ; rompt l'invariant « émission = travail de consensus ».

## 6. Notes d'implémentation

- **Prérequis stricts** : ADR 0002/0027/0031 éprouvés en banc n≥3 **avant** Phase 2.
- `vinx-core` : nouveau type `ValidatorCandidate { address, bond_atoms, entry_ts }` ;
  `GovernanceAction::VetoCandidate { candidate }` (vote d'un validateur actif) ;
  `GovernanceAction::UpdateMinValidatorBond { atoms }`.
- `vinx-state` : `pending_candidates: Vec<ValidatorCandidate>` dans `WorldState` ;
  `candidate_veto_votes: BTreeMap<Address, BTreeSet<Address>>` (candidat → validateurs ayant
  voté contre) ; logique d'admission à l'expiration ; logique de rejet au seuil 66 %.
- `vinx-state` : `S_perf` calculé en lecture depuis les blocs finalisés de `Chain` (pas de
  champ supplémentaire dans `WorldState`, données dérivées).
- `GenesisConfig` : `phase2_trigger` et `phase3_trigger` (seuil circulant ou ts) gravés ;
  `min_validator_bond_phase2`, `min_validator_bond_phase3` ; vecteur doré (ADR 0020).
- Dépendances : ADR 0002 (finalité), 0027 (jailing), 0028 (récompenses par époque),
  0031 (fork-choice), 0032 (garde-fous), 0033 (genèse multi-validateurs).
