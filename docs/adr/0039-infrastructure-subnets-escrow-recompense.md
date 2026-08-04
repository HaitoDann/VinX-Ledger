# ADR 0039 — Infrastructure de subnets : escrow bondé + racine de récompense

- **Statut :** Proposé (design à décider)
- **Catégorie :** Modules (par-dessus l'ADR 0001) · **Priorité :** 🟠 moyenne
- **Date :** Août 2026
- **Lié :** généralise la primitive d'ancrage (ADR 0010) ; **prérequis durs** ADR 0034
  (preuves d'inclusion) et ADR 0023 (adjudication de fraude) ; **ne touche pas** la courbe
  d'émission (ADR 0021) ni l'invariant de masse (ADR 0004) ; réutilise Merkle (ADR 0014/0020) ;
  synergie avec la sélection de comité par VRF (ADR 0029) ; anti-bloat cf. ADR 0026.

---

## 1. Contexte

L'ADR 0001 pose la doctrine : **VinX reste une monnaie pure**, et la variété applicative vit
dans des **modules hors-nœud** qui s'ancrent via un **bond** et des **commitments**. L'ADR 0010
(tranche 1, implémentée) donne le strict minimum : un opérateur enregistre un module, pose un
bond, et **ancre une racine d'état** (`anchor_head`). La L1 n'exécute, ne valide et ne comprend
**rien** de la logique interne du module.

Mais un module 0010 est **mono-opérateur** : c'est un rollup à un seul acteur qui ancre un
nombre. Pour qu'un développeur bâtisse une **vraie proposition de valeur** — un réseau de
*plusieurs* participants (mineurs, sondes, fournisseurs…) qui font un travail et **gagnent des
VINX** — il manque une seule capacité : **payer des participants multiples en VINX, sans
confiance, sur la base d'un travail jugé hors-chaîne.**

Aujourd'hui, un opérateur ne peut rémunérer ses participants qu'en leur envoyant des transferts
« à la main » — les participants doivent donc **lui faire confiance** pour payer honnêtement.
C'est l'écart que cet ADR comble, **sans** que la L1 ne se mette à juger le travail.

### Ce que cet ADR n'est PAS (doctrine, écartée explicitement)

Cet ADR **rejette le modèle Bittensor** comme mécanisme de protocole :

- **Pas d'émission dirigée vers les subnets.** L'émission reste immuable (0021) et coule aux
  producteurs de blocs. Un « pool d'émission subnet » exigerait soit un comité qui choisit les
  gagnants (tue la neutralité du fair launch), soit des tokens de subnet + AMM (tue « zéro
  spéculation »). Les subnets de VinX vivent de **vrais clients payants**, pas d'une subvention.
- **Pas de token par subnet, pas d'AMM, pas de scoring on-chain.** La L1 ne note jamais le
  travail ; elle détient des VINX et les libère contre une preuve. Tout le reste est hors-chaîne.

## 2. Décision

> **Un subnet = un module bondé (ADR 0010) + une réserve VINX en escrow + une racine de
> récompense cumulative, dont les participants tirent leur dû par preuve d'inclusion Merkle.**

La L1 gagne exactement trois opérations et deux champs d'état. Elle continue de n'exécuter
**aucune** logique de subnet : elle vérifie un droit, une preuve, et la conservation de l'escrow.

### 2.1 État ajouté (discipline d'append, ADR 0020)

Sur `ModuleEntry` (champs **appendés en dernier** ; migration `STORAGE_VERSION 10 → 11` qui
append l'encodage par défaut — un blob v10 est un préfixe strict d'un blob v11 ; snapshot JSON
via `serde(default)`) :

```rust
pub struct ModuleEntry {
    pub operator: Address,
    pub bond: Amount,
    pub anchor_head: Hash32,          // état du module (0010) — inchangé
    pub anchored_count: u64,          // inchangé
    // --- ADR 0039, appendés ---
    pub escrow: Amount,               // réserve VINX du subnet (alimentée par les clients)
    pub reward_root: Hash32,          // racine Merkle CUMULATIVE : addr → total VINX gagné à vie
}
```

Un registre borné des retraits déjà effectués, hors de `ModuleEntry` (une entrée par
participant réellement payé) :

```rust
// clé canonique (BTreeMap, jamais HashMap — ordre déterministe, ADR 0020)
pub module_claims: BTreeMap<(Hash32 /*module_id*/, Address), u128 /*déjà retiré, en atomes*/>,
```

### 2.2 Opérations ajoutées (variantes **appendées** à `ModuleOp`, indices 3/4/5)

`ModuleOp` est le payload du type de tx `AnchorState` (0x09). Les variantes existantes
(`Register`/`Anchor`/`Deregister`, indices 0/1/2) sont **inchangées** — les nouvelles vont
**en dernier** pour préserver les vecteurs dorés (ADR 0020).

```rust
enum ModuleOp {
    Register    { module_id, bond_atoms },              // 0 — 0010, inchangé
    Anchor      { module_id, anchor_head },              // 1 — 0010, inchangé
    Deregister  { module_id },                           // 2 — 0010, inchangé
    // --- ADR 0039 ---
    Deposit     { module_id, amount_atoms },             // 3 — abonde l'escrow
    SetRewardRoot { module_id, reward_root },            // 4 — opérateur only
    Claim       { module_id, cumulative_atoms, proof },  // 5 — participant tire son dû
}
```

### 2.3 Sémantique (validation **avant** mutation — une op rejetée ne consomme pas de nonce)

- **`Deposit`** — *n'importe qui* (client, sponsor, ou l'opérateur pour amorcer) déplace
  `amount_atoms` de son solde vers `escrow` du module. Débité du solde de l'émetteur + frais de
  base. **Circulation-neutre** : l'escrow fait partie de la circulation (il est dû aux
  participants), il change juste de détenteur. L'émetteur reste ≥ ED (ADR 0026).
- **`SetRewardRoot`** — **opérateur uniquement**. Remplace `reward_root` par une nouvelle racine
  Merkle **cumulative** (feuille = `(adresse, total_gagné_à_vie_en_atomes)`). L'opérateur
  **engage son bond** sur l'honnêteté de cette racine (une racine frauduleuse est contestable →
  ADR 0023). La L1 **ne vérifie pas** que la racine reflète un vrai travail — c'est l'affaire du
  subnet hors-chaîne. Paie le frais de base.
- **`Claim`** — *n'importe qui* peut soumettre pour un bénéficiaire. Vérifie la **preuve
  d'inclusion** (format ADR 0034/0014) de `(bénéficiaire, cumulative_atoms)` contre
  `reward_root`. Soit `w = module_claims[(module_id, bénéficiaire)]` (0 si absent) :
  - rejette si `cumulative_atoms ≤ w` (rien de nouveau à tirer) ;
  - soit `payout = cumulative_atoms − w` ; rejette si `payout > escrow` (**conservation dure**) ;
  - crédite `payout` au bénéficiaire, `escrow −= payout`, `module_claims[…] = cumulative_atoms`.
  - paie le frais de base (anti-spam) ; un **montant min de réclamation** (anti-poussière) borne
    la croissance de `module_claims`.

Le **modèle cumulatif** (à la MerkleDistributor) est ce qui rend le tout simple : chaque
nouvelle racine porte le *total à vie* de chacun ; un `Claim` ne verse que le **delta** non
encore tiré. Un participant peut donc réclamer quand il veut, en une ou plusieurs fois, et les
abondements successifs (`Deposit` + `SetRewardRoot`) s'empilent naturellement — sans bitmap ni
notion d'époque dans l'état L1.

## 3. Invariants (vérifiés en test, façon ADR 0004/0026)

1. **Conservation de l'escrow** : la somme des `payout` d'un module ne dépasse **jamais** le
   total déposé. Un `Claim` qui excéderait `escrow` est rejeté **avant** mutation.
2. **Anti-double-réclamation** : garanti par le modèle cumulatif + `module_claims` (on ne verse
   que `cumulative − déjà_retiré`, monotone croissant).
3. **Invariant de masse (ADR 0004)** : `Deposit` = solde → escrow ; `Claim` = escrow → solde.
   `circulating_supply` est **inchangé** dans les deux cas ; `circulation + Fonderie = 100 Md`
   tient. (À la différence du **bond**, verrouillé et toujours possédé par l'opérateur, l'escrow
   est *dû aux participants* — mais reste comptabilisé en circulation.)
4. **Aucune création** : le dommage maximal d'un opérateur malhonnête est **borné par l'escrow**
   qu'il ou ses clients ont financé. Il ne peut rien émettre ; il ne peut, au pire, que
   mal-diriger des fonds *déjà déposés* — et il risque son bond (0023).
5. **La L1 ne juge jamais le travail** : elle ne vérifie que droit d'opérateur, preuve
   d'inclusion, et conservation. Fidèle à l'ADR 0001.
6. **Anti-bloat (ADR 0026)** : frais de base + montant min de réclamation rendent le spam de
   `module_claims` économiquement absurde ; GC des entrées au `Deregister` (cf. §6).

## 4. Premier subnet de démonstration : balise d'aléa VRF

Le premier subnet doit **prouver la mécanique**, pas maximiser le chiffre d'affaires. Le bon
critère est donc la **vérifiabilité triviale du travail** — sinon on doit résoudre le problème
dur (preuve de fraude, 0023) dès le premier essai. On retient une **balise d'aléa public**
(*randomness beacon*) basée VRF :

- **Travail** : chaque participant publie, par round, `VRF(sk, round_seed)` + preuve. La balise
  du round = hash des sorties valides combinées.
- **Honnête par construction** : chaque sortie VRF **se vérifie elle-même** contre la clé
  publique du participant. Une `reward_root` malhonnête (créditer qui n'a pas contribué, omettre
  qui a contribué) est **trivialement prouvable** → c'est aussi le **banc d'essai idéal pour le
  chemin de fraude 0023**, avec des preuves propres. On valide `escrow + reward_root + Claim` en
  isolation, sans se battre en même temps avec la vérification.
- **Zéro dépendance, zéro matériel** : faire tourner un nœud suffit. Version simple **sans DKG**
  (sorties VRF indépendantes combinées par hash) — pas de setup à seuil.
- **Utilité réelle + synergie VinX** : loteries, ordonnancement équitable, sélection de jeux ;
  et à terme le beacon peut alimenter la **sélection de comité par VRF de l'ADR 0029** — le
  premier subnet rend la chaîne elle-même meilleure.

Le subnet **n°2** naturel est la **mesure de réseau / uptime décentralisé** (utilité produit
plus évidente), à attaquer *après* avoir durci le chemin de fraude, car sa vérification honnête
(accord croisé entre sondes, anti-spoofing géo) est justement le morceau dur.

## 5. Conséquences

**Positif**
- Débloque de **vrais subnets multi-participants payés en VINX**, sans salir le cœur : surface
  L1 = 2 champs + 3 ops, aucune logique de subnet exécutée.
- Crée une **demande organique** pour VINX (le carburant de l'écosystème) tout en préservant
  fair launch, monnaie pure, émission immuable, zéro spéculation.
- Réutilise tout l'existant : bond (0010), preuve Merkle (0034/0014), slashing (0023).

**Coûts / pièges**
- **Prérequis durs** : 0034 (la preuve du `Claim` *est* la preuve d'inclusion) et 0023 (sinon le
  bond ne dissuade pas une racine malhonnête). Et le **banc n≥3 stable** avant tout.
- L'honnêteté de la racine n'est aussi bonne que les preuves de fraude de 0023 : robuste pour le
  travail **vérifiable** (beacon, minage), délicate pour le **subjectif** (calcul/IA) — d'où le
  beacon en premier.
- Croissance de `module_claims` proportionnelle aux participants réels (bornée par frais + min
  claim + GC), pas au spam.

**Neutre**
- À `n=1` comme à `n≥3`, la mécanique est identique (elle ne dépend pas du consensus).

## 6. Questions ouvertes (à trancher dans cet ADR avant implémentation)

- **Escrow orphelin au `Deregister`** : que deviennent les fonds non réclamés ? *Piste
  privilégiée :* interdire `Deregister` tant que `escrow > 0`, ou time-lock puis retour au(x)
  financeur(s). À ne pas laisser à l'opérateur (conflit d'intérêt).
- **GC de `module_claims`** : purge des entrées d'un module au `Deregister` (après règlement).
- **Racine unique cumulative vs époques** : le cumulatif évite les époques dans l'état L1 ;
  garde-t-on une trace d'époque *hors-chaîne* seulement (recommandé) ?
- **Montant min de réclamation** : valeur (borne l'anti-poussière) — à graver comme constante
  testée (tripwire), façon `EXISTENTIAL_DEPOSIT_ATOMS`.
- **Qui peut `Deposit`** : ouvert à tous (recommandé, finance côté client) — confirmer.

## 7. Séquence de déblocage (l'ordre est non-négociable)

| # | Étape | Statut | Rôle |
|---|-------|--------|------|
| 0 | Banc n≥3 stable | chemin critique actuel | Prérequis absolu — ne rien empiler avant |
| 1 | ADR 0034 — DA + preuves d'inclusion | Proposé | La preuve du `Claim` **est** cette preuve |
| 2 | ADR 0023 — adjudication de fraude | Différé | Donne du sens au bond (racine malhonnête = slashable) |
| 3 | **Escrow + reward_root + Claim** (cet ADR) | Proposé | La brique « subnet payé en VINX » |
| 4 | Subnet de démonstration : **balise VRF** | à écrire | Vérification triviale → prouve le patron de bout en bout |

## 8. Alternatives écartées

- **Émission dirigée vers les subnets (modèle Bittensor / dTAO).** Rejeté : rouvre le fork
  écarté, exige comité-choisit (tue la neutralité fair launch) ou tokens+AMM (tue « zéro
  spéculation »), et se paie forcément sur l'émission des validateurs (0021/égalité). Doctrine.
- **Paiement direct par l'opérateur, sans escrow (ancrage seul).** Rejeté : les participants
  doivent faire confiance à l'opérateur pour payer. L'escrow + `Claim` rend le paiement **sans
  confiance** pour un coût L1 minuscule — c'est tout l'intérêt.
- **Bitmap de réclamation par époque** (au lieu du cumulatif). Rejeté : état par époque non
  borné, moins souple pour les abondements successifs. Le cumulatif est plus simple **et** plus
  robuste.
- **Streaming / canaux de paiement pour micro-paiement continu.** Reporté : puissant pour le
  stockage/calcul facturés à l'usage, mais surface L1 nettement plus grande. Possible **par
  dessus** cette brique, plus tard.

## 9. Notes d'implémentation

- **Migration** `STORAGE_VERSION 10 → 11` : append `escrow` (défaut 0) et `reward_root` (défaut
  `[0u8;32]`) sur `ModuleEntry` ; append la map `module_claims` (défaut vide). Blob v10 = préfixe
  strict d'un blob v11.
- **Variantes `ModuleOp`** : `Deposit`/`SetRewardRoot`/`Claim` **appendées en dernier** (indices
  3/4/5) → les vecteurs dorés de 0010 (Register/Anchor/Deregister) restent byte-identiques
  (ADR 0020). Ajouter des vecteurs dorés pour les 3 nouvelles.
- **Preuve d'inclusion** : réutilise la brique Merkle du `state_root` et le format light-client
  (ADR 0014), **le même** que celui standardisé par 0034 — un seul format à auditer.
- **Frais** : chaque op paie le forfait de base, crédité au producteur au règlement du bloc
  (comme 0010).
- **Tests** : conservation de l'escrow (property test somme des payouts ≤ dépôts), invariant de
  masse sur `Deposit`/`Claim` (0004), anti-double-claim, rejet `payout > escrow`, rejet sous le
  min claim, migration v10→v11 (préfixe), golden vectors des 3 nouvelles variantes.
