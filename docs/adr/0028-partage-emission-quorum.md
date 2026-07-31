# ADR 0028 — Partage de l'émission sur le quorum de finalité

- **Statut :** Proposé
- **Catégorie :** Tokenomics & frais / Consensus · **Priorité :** 🟠 moyenne
- **Date :** Juillet 2026
- **Liens :** modifie la distribution (pas la courbe) de l'émission (ADR 0021, respectée) ;
  s'appuie sur la finalité au quorum (ADR 0002) ; adresse la concentration early (contexte
  fair launch).

## Contexte

Aujourd'hui, `emit_work_reward` crédite **100 % de l'émission (et des frais) au seul
producteur** du bloc. Or :

1. **La co-signature est du travail non payé.** C'est pourtant elle qui donne la **finalité**
   (quorum, ADR 0002). On demande aux validateurs de co-signer sans les rémunérer → sous-
   incitation à la participation dont dépend la sécurité.
2. **L'émission est intégrée dans le temps** (`curve(now) − emitted`) : le producteur d'un
   bloc encaisse toute l'émission accumulée depuis le bloc précédent. Couplé au *winner-take-
   all* par bloc, cela rend les revenus **en grumeaux** et concentrés sur les proposeurs.
3. **Incitation à retarder** : plus le temps passe depuis le dernier bloc, plus la récompense
   du prochain producteur est grosse → micro-pression **contre la liveness**.
4. **Concentration early** (fair launch) : le halving front-load 50 % de la masse sur 8 ans ;
   si peu d'acteurs proposent tôt, ils captent l'essentiel. Le *winner-take-all* amplifie.

## Décision proposée

**Répartir l'émission de chaque bloc entre son proposeur et les co-signataires qui l'ont
finalisé**, au lieu de tout donner au proposeur. Cela **ne touche pas la courbe d'émission**
(ADR 0021 immuable) : le **total émis par unité de temps est identique** ; seule la
**distribution entre validateurs** change.

### Où et quand créditer

Les co-signatures arrivent **après** la production (le bloc est diffusé, puis les
`BlockCoSignature` sont gossipées). Au moment de `settle_block`, le proposeur n'a que sa
propre signature. Donc le partage impose de **créditer l'émission à la finalisation**, pas à
la production :

> Quand un bloc `H` atteint le quorum et devient **finalisé** (avance de `finalized_height`,
> prefix-closed, ADR 0002), on distribue l'émission accumulée qui lui est attribuée :
> - `PROPOSER_SHARE_BPS` au proposeur (récompense de la mise en ordre + inclusion),
> - le reste **à parts égales entre les co-signataires valides** du bloc finalisant.

La finalité étant *prefix-closed*, chaque bloc finalise dans l'ordre → attribution par bloc
sans ambiguïté. L'émission est toujours **forgée depuis la Fonderie** ; seul le **nombre de
bénéficiaires** change. L'invariant de masse (ADR 0004) tient (Fonderie −x, N comptes +x).

### Frais

Les **frais** peuvent suivre la même règle (partagés) ou rester au proposeur (il assume
l'inclusion et la construction du bloc). Recommandation tranche 1 : **frais au proposeur,
émission partagée** — les frais rémunèrent le travail d'inclusion, l'émission rémunère la
sécurité collective (co-signatures).

### Répartition égalitaire

Le partage entre co-signataires est **à parts égales** (une signature = une part), **pas
pondéré par le bond** — cohérent avec l'ethos VinX (`test_emission_is_not_weighted_by_bond`) :
aucun avantage aux baleines. Le `PROPOSER_SHARE_BPS` est un paramètre de politique (p.ex.
20–40 %), **gouvernable** (contrairement à la courbe) car il ne change pas la masse.

## Modèle

- L'émission d'un bloc = `curve(ts_H) − emitted_avant_H`, forgée à la finalisation de `H`.
- `part_proposeur = émission × PROPOSER_SHARE_BPS / 10_000`.
- `reste = émission − part_proposeur`, réparti en `reste / k` à chacun des `k` co-signataires
  (le résidu de division entière va au proposeur — déterministe, pas de perte).
- Crédits soumis au dépôt existentiel (ADR 0026) : un validateur est bondé (`staked > 0`) donc
  exempté du plancher, jamais poussière.

## Conséquences

**Positif**
- Rémunère la **participation à la finalité** → renforce la sécurité qu'on demande déjà.
- **Lisse** les revenus sur l'ensemble des signataires → meilleure décentralisation, atténue
  la concentration early (Q4) **sans** toucher l'émission immuable.
- **Réduit l'incitation à retarder** : la manne d'un long silence se partage, elle n'enrichit
  plus un seul acteur.
- Synergie avec l'ADR 0027 : rater ses co-signatures réduit naturellement le revenu (incitation
  douce à la fiabilité, sans peine subjective).

**Coûts / pièges**
- **Déplace le moment du crédit** de la production vers la finalisation → change quand
  l'émission entre en circulation ; à réconcilier avec `finalized_height`, l'invariant de
  masse et les tests. Changement **consensus-critique**.
- À `n = 1` (mono-validateur, finalité immédiate), le proposeur = unique co-signataire → il
  reçoit tout : comportement identique à aujourd'hui (pas de régression au bootstrap).
- Un bloc non finalisé n'émet pas encore : l'émission « en attente » doit être suivie
  proprement (idempotence, pas de double crédit à la finalisation).

## Alternatives écartées

- **Garder 100 % au proposeur** : rejeté — sous-paie la finalité, concentre, incite à retarder.
- **Pondérer les parts par le bond** : rejeté — casse l'égalitarisme volontaire de VinX,
  favorise les baleines.
- **Toucher la courbe d'émission pour lisser le début** : rejeté — l'ADR 0021 l'a gravée
  immuable ; c'est une feature de confiance. Ce ADR agit sur la **distribution**, jamais sur la
  masse.
- **Créditer à la production avec les co-sigs connues d'avance** : impossible — les co-sigs
  arrivent après la diffusion du bloc.

## Notes d'implémentation

- `vinx-core` : `PROPOSER_SHARE_BPS` (gouvernable via `UpdateFeeFloor`-like ou nouvelle action).
- `vinx-state` : déplacer l'attribution de l'émission de `settle_block` (production) vers le
  point de finalisation ; répartir sur `block.signatures` valides ; suivre l'émission en
  attente par bloc pour éviter tout double crédit ; conserver l'invariant de masse (ADR 0004).
- Tests : somme des crédits = émission forgée (conservation), parts égales + résidu au
  proposeur, `n=1` inchangé, pas de double crédit, exemption ED des validateurs (ADR 0026),
  idempotence à la finalisation.
- Dépendance : nécessite la finalité prefix-closed (ADR 0002) et le banc n≥2 pour valider le
  flux co-signatures→crédit.
