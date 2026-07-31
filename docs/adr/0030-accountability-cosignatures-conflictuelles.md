# ADR 0030 — Accountability des co-signatures conflictuelles (sûreté de finalité)

- **Statut :** Proposé
- **Catégorie :** Sécurité / Consensus & finalité · **Priorité :** 🔴 haute (propriété de
  sûreté, pas une amélioration)
- **Date :** Juillet 2026
- **Liens :** complète le slashing d'équivocation (ADR 0003) ; rend *accountable* la finalité
  au quorum (ADR 0002) ; réutilise la primitive de slashing existante.

## Contexte

La finalité de VinX repose sur un **quorum de co-signatures** (ADR 0002) : un bloc est final
quand ≥ `quorum` validateurs l'ont co-signé. La question de sûreté fondamentale d'un tel
système est : **que se passe-t-il si deux blocs différents sont finalisés à la même hauteur ?**
C'est une **double-finalité** — un fork irréversible, le pire échec possible.

Pour qu'une double-finalité soit *dissuadée économiquement*, il faut qu'elle soit
**accountable** : si elle survient, un ensemble de validateurs doit être **prouvablement
fautif** et **slashable** (argument à la Casper : deux quorums qui se recoupent ⇒ au moins
`2·quorum − N` validateurs ont **co-signé les deux blocs conflictuels** — une faute prouvable).

### Ce qui existe déjà (vérifié dans le code)

Bonne surprise : la **primitive de slashing on-chain gère déjà le cas général**.
`apply_slash_validator` vérifie une `SlashEvidence { header_a, header_b, sig_a, sig_b }` et
exige uniquement :

1. même hauteur, hachages différents ;
2. `sig_a.validator == sig_b.validator == cible` (le **même** validateur a signé les deux) ;
3. les deux signatures **cryptographiquement valides** sur leurs en-têtes respectifs.

Il **n'exige pas** que la cible soit le *proposeur*. Donc **tout validateur qui co-signe deux
en-têtes conflictuels à la même hauteur est déjà slashable** — la double-proposition (ADR
0003) n'en est qu'un cas particulier.

### Le vrai trou : la détection

Ce qui manque est **entièrement côté détection P2P**. Aujourd'hui (`report_equivocation`,
détection à la réception de bloc) le nœud ne guette que la **double-proposition** : un *bloc*
différent du **même proposeur** à une hauteur déjà scellée. Il ne surveille **pas** un
validateur qui **co-signe deux blocs conflictuels** via le flux `BlockCoSignature`. C'est
exactement le « reste » noté dans l'ADR 0003 (« détection via co-signatures conflictuelles —
nécessite de conserver les deux en-têtes signés »).

## Décision proposée

Fermer la boucle **côté détection**, en réutilisant la primitive de slashing telle quelle.

1. **Rétention des en-têtes conflictuels.** À une hauteur non encore finalisée, un nœud peut
   voir plusieurs en-têtes valides concurrents (proposeur + backup, ou proposeur malveillant).
   Conserver, par hauteur, les en-têtes distincts observés (bornés : la fenêtre non-finalisée
   est courte, cf. finalité prefix-closed).
2. **Index des co-signatures par (hauteur, validateur).** À chaque `BlockCoSignature` reçue,
   enregistrer `(height, validator) → hash signé`. Une co-signature porte le hash du bloc ; on
   la relie à l'en-tête retenu correspondant.
3. **Assemblage & auto-report.** Si le **même validateur** apparaît avec **deux hachages
   différents à la même hauteur**, on tient les deux en-têtes retenus + ses deux
   `BlockSignature` ⇒ on construit la `SlashEvidence` et on la soumet/gossipe automatiquement
   (même chemin que `report_equivocation`, réservé aux nœuds validateurs qui touchent la
   prime). La preuve est vérifiée cryptographiquement on-chain → **aucun faux positif
   possible**.

Le slashing lui-même (100 %, prime de 10 % au rapporteur, reste fondu — ADR 0003) est
**inchangé** : la cible « valide deux en-têtes conflictuels » est déjà acceptée par
`apply_slash_validator`.

## Modèle — la garantie d'accountability

Avec `quorum = ⌈2N/3⌉+…` (selon le réglage), deux blocs finalisés à la même hauteur
impliquent que **≥ `2·quorum − N` validateurs ont co-signé les deux** ⇒ chacun est porteur
d'une `SlashEvidence` valide ⇒ **une fraction bornée du bond total est slashable**. La
double-finalité n'est donc jamais « gratuite » : elle a un coût économique prouvable et
récupérable. C'est ce qui transforme la finalité de 0002 d'une finalité *optimiste* en une
finalité *accountable*.

## Conséquences

**Positif**
- Ferme la faute BFT la plus grave (double-sign de finalité) — sûreté, pas confort.
- Coût d'implémentation **faible** : la partie consensus-critique (vérif + slashing) existe
  déjà ; on n'ajoute que de la **détection** hors chemin de transition d'état.
- Aucune nouvelle structure de preuve : réutilise `SlashEvidence` (déjà à vecteurs dorés,
  ADR 0020).

**Coûts / pièges**
- **Rétention mémoire** des en-têtes/co-sigs conflictuels : à **borner** (par hauteur, purge
  sous `finalized_height`) pour ne pas ouvrir un vecteur de bloat/DoS mémoire (cohérent avec
  ADR 0022).
- **Course prime/rapport** : plusieurs nœuds peuvent rapporter la même équivocation ; le nonce
  et le « déjà slashé » gèrent l'idempotence (comme aujourd'hui pour la double-proposition).
- Ne **prévient** pas la double-finalité — il la **dissuade/punit** a posteriori. La prévention
  relève du fork-choice (ADR 0031) et de la finalité formelle (ADR 0002).

## Alternatives écartées

- **Nouveau type de preuve dédié aux co-sigs** : inutile — `SlashEvidence` couvre déjà le cas
  (signataire ≠ proposeur autorisé).
- **Slashing partiel pour co-sign conflictuel** : rejeté — c'est une faute de sûreté délibérée
  (on ne co-signe pas deux blocs par accident) ; 100 % comme l'équivocation de proposition.
- **Ne rien faire** : rejeté — laisse la finalité non-accountable, ce qui est un défaut de
  sûreté, pas une lacune de confort.

## Notes d'implémentation

- `vinx-node/p2p` : structure de rétention `(height → {header}, (height,validator) → hash)`
  bornée et purgée sous `finalized_height` ; détection de conflit → assemblage `SlashEvidence`
  → `report_equivocation`-like. **Aucun** changement à `apply_slash_validator`.
- Tests : deux co-sigs conflictuelles d'un même validateur → preuve assemblée & acceptée ;
  co-sigs sur le même hash → pas de faute ; borne mémoire respectée ; idempotence multi-
  rapporteurs.
- Dépendance : `finalized_height` (ADR 0002) pour la purge ; s'articule avec le fork-choice
  (ADR 0031) qui décide quel bloc est canonique **avant** finalité.
