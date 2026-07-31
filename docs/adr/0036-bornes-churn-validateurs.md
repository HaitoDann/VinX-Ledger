# ADR 0036 — Bornes de churn du set de validateurs (file de sortie)

- **Statut :** Proposé
- **Catégorie :** Consensus & finalité / Sécurité · **Priorité :** 🟠 moyenne
- **Date :** Juillet 2026
- **Liens :** étend le cap d'unbonds par compte (ADR 0009) au niveau **agrégat** ; protège la
  liveness/finalité (ADR 0002) ; complète le jailing (ADR 0027).

## Contexte

L'ADR 0009 borne les déliaisons **par compte** (`MAX_PENDING_UNBONDS_PER_ACCOUNT`) — un
anti-spam d'état individuel. Mais **rien ne borne le churn agrégé** du set de validateurs :

- une **sortie massive simultanée** (beaucoup de validateurs unstake/quittent en même temps)
  fait **chuter le set** d'un coup → le **quorum** de finalité peut ne plus être atteignable,
  ou la sécurité (fraction honnête requise) s'effondrer brutalement ;
- symétriquement, une **entrée massive** (une fois l'admission permissionless, ADR 0033) peut
  diluer/instabiliser la rotation.

C'est un risque **de sécurité et de liveness** distinct du spam d'état : même des validateurs
parfaitement honnêtes qui partent tous ensemble (panique, changement de reward ailleurs)
peuvent casser le consensus. Les systèmes PoS matures (Ethereum, Cosmos) bornent tous le
**taux de changement** du set pour cette raison (churn/exit queue).

## Décision proposée

Introduire une **file de sortie (et d'entrée) bornée** : le set de validateurs ne peut changer
qu'à un **taux maximal** par fenêtre, de façon **déterministe**.

1. **Taux de sortie borné** : au plus `MAX_VALIDATOR_EXITS_PER_WINDOW` validateurs quittent
   effectivement le set actif par fenêtre (de hauteurs ou de temps). Les demandes au-delà sont
   **mises en file** et traitées aux fenêtres suivantes, dans un ordre **déterministe** (FIFO
   par hauteur de demande, départage par adresse).
2. **Bond retenu jusqu'à la sortie effective** : tant qu'un validateur est dans la file, son
   bond **reste verrouillé et slashable** (cohérent avec le délai d'unbonding de l'ADR 0009 —
   la file *est* une généralisation du délai, appliquée à l'échelle du set).
3. **Plancher de set** (lien ADR 0032) : la file ne peut jamais faire descendre le set actif
   sous un **plancher de sécurité** — une sortie qui violerait le plancher est refusée/retardée
   jusqu'à ce qu'une entrée compense.
4. **Taux d'entrée borné** (une fois l'admission permissionless, ADR 0033) : symétrique, pour
   éviter qu'une vague d'entrées ne déstabilise la rotation d'un coup.

## Modèle

- Une **file** (`exit_queue`, `entry_queue`) dans le meta `WorldState`, dérivée
  déterministiquement des tx (comme `pending_unbonds`) → identique sur tous les nœuds, hors
  `state_root`.
- Le **quorum** et `leader_at` opèrent sur le set **actif** (post-file), dont l'évolution est
  bornée et prévisible → pas de saut brutal de finalité.
- Interaction jailing (ADR 0027) : un jail est un retrait **temporaire** (pas une sortie de
  set) et **ne passe pas** par la file d'exit ; la file concerne les sorties **volontaires**
  (unstake sous le bond) et éventuellement les évictions définitives.

## Conséquences

**Positif**
- Protège **liveness et sécurité** contre un effondrement/une dilution brutale du set.
- Généralise proprement le délai d'unbonding (0009) du compte au **set entier**.
- Rend l'évolution du set **prévisible** — bon pour la rotation, la finalité et la confiance.

**Coûts / pièges**
- Un validateur qui veut partir peut être **retardé** (file) — c'est le prix de la stabilité ;
  à calibrer pour ne pas *piéger* les fonds trop longtemps (tension liberté de sortie ⟂
  stabilité, à l'image de la queue de sortie d'Ethereum).
- Nouveaux champs meta (files) → **bump de version + migration append** (0010/0011).
- Le **plancher de set** doit être cohérent avec l'ADR 0032 (garde-fous gouvernance) — les deux
  se conçoivent ensemble (qui fixe le plancher, immuable ou gouvernable ?).
- Consensus-critique (le set actif borné entre dans la rotation/quorum) → banc n≥3.

## Alternatives écartées

- **Aucune borne agrégée (statu quo)** : rejeté — laisse un risque de liveness/sécurité réel
  sur une sortie coordonnée, même honnête.
- **Interdire les sorties multiples** (au lieu de les mettre en file) : rejeté — trop rigide,
  piège les fonds ; la file borne le *taux* sans bloquer la sortie.
- **Borner uniquement les sorties malveillantes** : impossible à distinguer déterministiquement
  d'une sortie honnête → on borne le **taux**, indépendamment de l'intention.

## Notes d'implémentation

- `vinx-core/amount.rs` : `MAX_VALIDATOR_EXITS_PER_WINDOW`, `MAX_VALIDATOR_ENTRIES_PER_WINDOW`,
  taille de fenêtre, plancher de set (partagé avec 0032).
- `vinx-state` : `exit_queue`/`entry_queue` en meta ; traitement déterministe par fenêtre au
  règlement de bloc ; `RemoveValidator`/unstake sous bond → mise en file ; bond retenu jusqu'à
  sortie effective ; migration meta.
- Tests : sortie massive étalée sur plusieurs fenêtres, plancher jamais franchi, ordre
  déterministe, bond slashable dans la file, cohérence quorum/rotation.
- Dépendances : généralise ADR 0009 ; plancher partagé avec ADR 0032 ; s'articule avec
  l'admission permissionless (ADR 0033) et le jailing (ADR 0027).
