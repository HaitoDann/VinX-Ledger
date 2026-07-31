# ADR 0027 — Fiabilité & jailing des validateurs

- **Statut :** Proposé
- **Catégorie :** Consensus & finalité · **Priorité :** 🟠 moyenne
- **Date :** Juillet 2026
- **Liens :** complète la finalité (ADR 0002) et le slashing d'équivocation (ADR 0003) ;
  distinct du slashing (faute prouvée) — ici on gère la **fiabilité**, pas la malveillance.

## Contexte

Le planning de leader est un **round-robin par hauteur** (`leader_at(H) = validators[H % N]`),
mais **advisory** : si le leader prévu ne produit pas dans la fenêtre de timeout, un
**backup** produit à sa place (slot-skip, déjà présent). Ce qui manque : **aucune trace
d'état** de qui a manqué son tour, et **aucun mécanisme** pour écarter un validateur
chroniquement lent ou hors-ligne. Un validateur mort dégrade la liveness à chaque fois que
son tour revient (le réseau attend le timeout, puis le backup) sans jamais être retiré.

Le slashing (ADR 0003) ne s'applique qu'à une **faute prouvée** (équivocation) — pas à la
simple indisponibilité. Il faut un mécanisme distinct, **non destructif** : le **jailing**
(retrait temporaire de la rotation), à la Tendermint/Cosmos.

### La contrainte non négociable : déterminisme

Une note qui **influence le consensus** (rotation, quorum, récompenses) doit être une
**fonction déterministe de faits on-chain**, calculée identiquement par tous les nœuds —
sinon deux nœuds jailent des validateurs différents → **fork**. Corollaire :

- ✅ **Peuvent** piloter le consensus : faits dérivables de la séquence canonique de blocs —
  **slots manqués** (proposeur effectif ≠ leader prévu) et **co-signatures absentes** sur une
  fenêtre de blocs finalisés.
- ❌ **Ne peuvent jamais** entrer dans le consensus : latence, « vitesse », uptime *vu par un
  pair* — **subjectifs** (chaque nœud mesure autre chose). Ils restent dans le peer-scoring
  P2P (ADR 0022) et le monitoring off-chain.

## Décision proposée

### Attribution d'un « slot manqué » (spécificité block-on-demand)

En on-demand, il n'y a **pas de slots temporels fixes** : la hauteur n'avance que lorsqu'un
bloc est produit (travail en attente ou heartbeat). Donc un « slot manqué » se définit
proprement : **le bloc `H` existe et son proposeur ≠ `leader_at(H)` sur le set actif**. Cela
n'arrive que si un bloc était **dû** et que le leader prévu ne l'a pas pris dans le timeout —
un vrai manquement. Un créneau *inactif* (pas de travail) ne fait pas avancer la hauteur donc
n'est **jamais** compté comme manqué. L'attribution est ainsi 100 % déterministe, sans horloge.

### État par validateur (consensus meta)

```
ValidatorReliability {
    missed_proposals: u32,          // manquements consécutifs (remis à 0 sur une prod réussie)
    cosign_window: BitWindow,       // participation aux N derniers blocs finalisés
    jailed_until: Option<u64>,      // hauteur (ou ts) de sortie de prison au plus tôt
}
```

Rangé dans le meta `WorldState` (comme `pending_unbonds`), dérivé déterministiquement de la
séquence de blocs — donc identique sur tous les nœuds, hors `state_root` (cohérent avec
l'existant).

### Règles

1. **Manquement de proposition** : à l'application du bloc `H`, si `proposeur(H) ≠
   leader_at(H)` (set actif), `leader_at(H).missed_proposals += 1` ; une production réussie
   remet son compteur à 0.
2. **Manquement de co-signature** : à la finalisation du bloc `H` (quorum atteint, ADR 0002),
   chaque validateur actif **absent** de `signatures` voit son bit de participation à 0 dans
   la fenêtre glissante.
3. **Jailing** : quand `missed_proposals ≥ MAX_MISSED_PROPOSALS` **ou** le taux de
   participation sur la fenêtre `< MIN_COSIGN_PARTICIPATION_BPS`, le validateur est **jailé** :
   - retiré de la **rotation active** (`leader_at` itère sur les validateurs non jailés) ;
   - **exclu du numérateur ET du dénominateur du quorum** pendant l'emprisonnement (sinon un
     jailé bloquerait la finalité) ;
   - son **bond reste verrouillé et slashable** — le jailing n'est pas une sortie.
4. **Unjail** : après un `cooldown` (hauteur/ts), le validateur soumet une tx `Unjail`
   (opérateur uniquement) qui remet les compteurs à 0 et le réintègre à la rotation. Le
   cooldown empêche le battement (jail↔unjail).

### Garde-fous anti-grief

Un adversaire capable de retarder tes co-signatures pourrait te faire jailer. Donc :
- pénaliser **uniquement** sur des faits **attribuables sans ambiguïté** (ta signature absente
  d'un bloc **finalisé**, ton slot pris par un backup) ;
- des **seuils indulgents** (fenêtre longue, `MAX_MISSED_PROPOSALS` de l'ordre de plusieurs
  tours) ;
- **pas de réduction de récompense** ad hoc : un jailé ne produit plus donc gagne moins
  mécaniquement (et, si l'ADR 0028 est adoptée, rater ses co-signatures réduit déjà son
  revenu). On évite d'empiler une double peine subjective.

## Modèle

Invariant de rotation : `leader_at` et la sélection du backup opèrent sur le **set actif**
(non jailés). La définition doit être **totale et déterministe** même quand le set change —
c'est le même prérequis que le view-change formel listé dans l'ADR 0002 ; ce ADR le concrétise
côté état.

## Conséquences

**Positif**
- Liveness : un validateur mort est écarté au lieu de faire ramer chaque tour.
- Incite à la disponibilité **sans** subjectivité ni fork-risk.
- Réutilise le slot-skip existant ; complément non destructif du slashing (ADR 0003).

**Coûts / pièges**
- Consensus-critique : l'état de fiabilité et l'exclusion du quorum entrent dans la transition.
  Exige le **banc 3-validateurs** pour être validé (comme le view-change 0002).
- L'ajustement dynamique du **dénominateur de quorum** est délicat (un jail/unjail change le
  seuil de finalité) — à spécifier et tester rigoureusement.
- Nouveaux champs meta → **bump de version + migration append** (technique ADR 0010/0011).

## Alternatives écartées

- **Note de latence/uptime dans le consensus** : rejeté — subjectif → fork.
- **Slasher l'indisponibilité** (destruction de bond) : rejeté pour la tranche 1 — trop
  punitif pour une panne honnête ; le jailing (réversible) est proportionné. Un slashing léger
  d'inactivité prolongée reste envisageable plus tard.
- **Ne rien faire** : rejeté — un validateur mort dégrade la liveness indéfiniment.

## Notes d'implémentation

- `vinx-core` : constantes `MAX_MISSED_PROPOSALS`, `MIN_COSIGN_PARTICIPATION_BPS`,
  `UNJAIL_COOLDOWN`, fenêtre de participation ; tx `Unjail` (nouveau `TransactionType` appendé,
  ou `GovernanceAction`/`ModuleOp`-like).
- `vinx-state` : `ValidatorReliability` par validateur, mise à jour à l'application et à la
  finalisation des blocs ; `leader_at`/quorum sur le set actif ; migration meta.
- Tests : attribution déterministe des manquements, non-comptage des créneaux inactifs, jail au
  seuil, quorum recalculé, unjail après cooldown, non-grief sur retard de co-signature.
- Dépendance : s'appuie sur la finalité prefix-closed de l'ADR 0002 pour définir « bloc
  finalisé » et l'ensemble des co-signataires.
