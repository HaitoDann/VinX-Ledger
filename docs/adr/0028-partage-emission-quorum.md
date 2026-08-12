# ADR 0028 — Partage de l'émission par époque

- **Statut :** Accepté (décision de design) — **non implémenté** à ce jour.
- **Catégorie :** Tokenomics & frais / Consensus · **Priorité :** 🟠 moyenne
- **Date :** Juillet 2026 — révisé Août 2026 (passage au modèle par époque)
- **Liens :** modifie la distribution (pas la courbe) de l'émission (ADR 0021, respectée) ;
  s'appuie sur la finalité au quorum (ADR 0002) ; adresse la concentration early (contexte
  fair launch) ; s'articule avec l'admission Open PoA (ADR 0038).

---

## 1. Contexte

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
5. **Coût en crédits à l'échelle** : créditer individuellement N validateurs à chaque bloc
   (N pouvant croître jusqu'à 101 avec ADR 0038) génère des opérations d'état par bloc
   proportionnelles à la taille du set → coût prohibitif à grande échelle.

## 2. Décision

**Distribuer l'émission par époque** — une fenêtre temporelle définie — au lieu de bloc
par bloc. Les **frais** restent crédités au producteur **immédiatement** à chaque bloc.

> La courbe d'émission (ADR 0021) est **inchangée** : la masse totale par unité de temps
> est identique. Ce que change cet ADR : le *rythme et la répartition* de la distribution,
> pas la *masse créée*.

### 2.1 Époque de distribution

Une **époque** est une fenêtre de durée `EPOCH_DURATION_SECS` (paramètre gouvernable,
valeur indicative : 3 600 s = 1 heure). L'émission accumulée sur l'époque est distribuée
en **une seule passe** à la clôture.

#### Accumulation (à chaque bloc finalisé dans l'époque)

Pour chaque bloc `B` dont les co-signatures atteignent le quorum dans l'époque courante :

```
block_emission_B  = curve(ts_B) − emitted_avant_B     (nouvellement émis, ADR 0040)
part_proposeur_B  = block_emission_B × PROPOSER_SHARE_BPS / 10_000
pot_cosignataires += block_emission_B − part_proposeur_B

proposer_credits[proposeur_de_B] += part_proposeur_B
pour chaque co-signataire v valide du bloc B :
    cosign_count[v] += 1
```

#### Distribution (à la clôture de l'époque)

Quand `timestamp_bloc_courant ≥ epoch_start_ts + EPOCH_DURATION_SECS` :

1. Créditer chaque proposeur depuis `proposer_credits[addr]` (accumulé au fil des blocs).
2. Calculer `total_cosign_events = Σ cosign_count[v]`.
3. Si `total_cosign_events > 0` : chaque co-signataire `v` reçoit
   `floor(pot_cosignataires × cosign_count[v] / total_cosign_events)`.
4. Le **résidu** (troncature entière) va au validateur ayant le plus de co-signatures dans
   l'époque — déterministe, zéro perte de atoms.
5. Réinitialiser : `pot_cosignataires = 0`, `proposer_credits = {}`, `cosign_count = {}`,
   `epoch_start_ts += EPOCH_DURATION_SECS`.

L'invariant de masse (ADR 0040) tient : `emitted_atoms +x`, `circulating +x`, avec x = émission
totale de l'époque. `remaining_supply` décroît de façon monotone sur chaque époque.

### 2.2 Frais — inchangés, immédiats

Les **frais de transaction** restent crédités au producteur du bloc **immédiatement**, hors
mécanisme d'époque. Ils rémunèrent le travail d'**inclusion** (sélection, ordering, infra)
qui revient au proposeur. Seule l'**émission** rémunère la sécurité collective (co-signatures)
et passe donc par l'époque.

### 2.3 Répartition proportionnelle à la participation

La part d'un co-signataire est **proportionnelle au nombre de blocs co-signés** dans l'époque.
Un validateur ayant co-signé 90 % des blocs gagne 9× plus qu'un validateur en ayant co-signé
10 %. Ce n'est pas une égalité stricte entre validateurs mais une égalité **par acte de
co-signature** — chaque co-sign vaut la même fraction du pot, indépendamment de qui le pose.

> **Cohérence avec ADR 0027 (jailing)** : un validateur jailé ne co-signe plus → son
> `cosign_count` est zéro → il ne reçoit aucune émission de l'époque. Incitation douce
> à la disponibilité, sans slash économique du bond.

### 2.4 Paramètres

| Paramètre | Gouvernable ? | Valeur indicative | Rôle |
|---|---|---|---|
| `EPOCH_DURATION_SECS` | Oui | 3 600 s (1 h) | Durée d'une époque |
| `PROPOSER_SHARE_BPS` | Oui | 2 000 (20 %) | Part du proposeur dans l'émission du bloc |

Les deux paramètres sont **gouvernables** (ne touchent pas la courbe d'émission, ADR 0021
immuable). Une valeur de `PROPOSER_SHARE_BPS = 10_000` revient au modèle actuel (100 % au
proposeur) ; `0` donne tout aux co-signataires — les deux extrêmes sont valides.

### 2.5 Comportement à n = 1 (bootstrap)

À un seul validateur, il est à la fois le seul proposeur et le seul co-signataire → il reçoit
l'intégralité de l'émission de l'époque. Comportement **identique à aujourd'hui** — pas de
régression au bootstrap.

## 3. Conséquences

**Positif**
- **Rémunère la finalité** → renforce l'incitation à co-signer, qui est le cœur du consensus.
- **Lisse les revenus** : plus de grumeaux géants sur un seul proposeur ; distribution régulière.
- **Réduit l'incitation à retarder** : la manne d'un long silence se dilue sur toute l'époque.
- **Moins de transactions de crédit** : une seule passe par époque, quelle que soit la taille
  du set (O(1) par époque au lieu de O(N) par bloc).
- **Synergie avec ADR 0038 (Open PoA)** : plus le set grandit, plus l'époque est efficace.

**Coûts / pièges**
- **Champs d'état supplémentaires** dans `WorldState` (pot, credits, cosign counts, epoch ts) →
  changement **consensus-critique** ; bump de version de stockage.
- **Délai de paiement** : les co-signataires attendent la fin de l'époque (≤ 1 h en config
  par défaut). Acceptable pour un réseau de paiement ; à documenter pour les opérateurs.
- **Bord d'époque** : le passage d'époque est déclenché par le timestamp du premier bloc
  qui dépasse `epoch_start_ts + EPOCH_DURATION_SECS` — déterministe et identique sur tous les
  nœuds. Si aucun bloc n'est produit pendant plusieurs époques, la clôture de toutes les
  époques vides est traitée en séquence au bloc suivant (époques vides = distribution nulle).
- **Idempotence** : la clôture d'époque doit être idempotente et rejouable (replay de sync).

## 4. Alternatives écartées

- **Partage par bloc, à la finalisation** (version initiale de cet ADR) : rejeté — O(N)
  crédits par bloc → coût prohibitif à grande échelle (N jusqu'à 101) ; revenus toujours
  en grumeaux à chaque finalisation.
- **Garder 100 % au proposeur** : rejeté — sous-paie la finalité, concentre l'émission,
  incite à retarder.
- **Pondérer les parts par le bond** : rejeté — casse l'égalitarisme volontaire de VinX,
  favorise les baleines.
- **Commerce Pool** (redistribution vers l'activité transactionnelle) : rejeté — gameable
  (volume artificiel), ne crée pas de valeur réelle, sort l'émission du périmètre de la
  sécurité du consensus.
- **Toucher la courbe d'émission pour lisser le début** : rejeté — ADR 0021 l'a gravée
  immuable ; ce ADR agit sur la **distribution**, jamais sur la **masse**.

## 5. Notes d'implémentation

- `vinx-core` : constantes `EPOCH_DURATION_SECS` + `PROPOSER_SHARE_BPS` (gouvernables).
- `vinx-state` : ajouter à `WorldState` :
  `epoch_dist_start_ts: u64`, `epoch_dist_emission_pot: u128`,
  `epoch_dist_proposer_credits: BTreeMap<Address, u128>`,
  `epoch_dist_cosign_counts: BTreeMap<Address, u32>`.
  `settle_block` → accumuler (plus créditer immédiatement pour l'émission).
  `close_epoch_if_due` → distribuer puis réinitialiser ; appelé dans la boucle de production
  et à la réception P2P (avant ou après `settle_block`, ordre déterministe à préciser).
- Bump `STORAGE_VERSION` : `epoch_dist_*` appendés → migration append (préfixe strict).
- Tests : conservation (Σ crédits = émission de l'époque), proportionnalité co-signatures,
  `n=1` inchangé, pas de double crédit, bord d'époque à timestamp exact, époques vides,
  idempotence, exemption ED des validateurs (ADR 0026).
- Dépendances : ADR 0002 (finalité), ADR 0038 (Open PoA, synergique pour l'échelle).
