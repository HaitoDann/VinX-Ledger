# ADR 0071 — Verrou de vote persistant : une signature par hauteur

- **Statut :** Implémenté 🔧 — audit de sécurité de septembre 2026 (findings VX-RED-003, VX-RED-007)
- **Date :** Septembre 2026
- **Portée :** Consensus — invariant de vote d'un validateur honnête
- **Décideur :** VinX Labs
- **Complète :** ADR 0003 (slashing de l'équivocation), ADR 0030 (accountability des co-signatures conflictuelles)
- **Crates :** `vinx-node` (`src/storage.rs`, `src/p2p/mod.rs`, `src/node.rs`)

---

## 1. Contexte

VinX disposait d'une **détection** d'équivocation (`Chain::record_signature`, ADR 0003) mais
d'aucune **prévention**. Le chemin de co-signature signait puis enregistrait, et jetait le
résultat :

```rust
let bls_sig = bls_sk.sign(&block_hash);
// ... publication sur gossip ...
c.record_signature(local_addr, height, block_hash);   // résultat ignoré
```

`record_signature` retourne pourtant `true` lorsque le validateur a déjà signé un **hash
différent** à la même hauteur. L'information existait, arrivait trop tard, et n'était
utilisée que comme télémétrie.

**Conséquence :** un attaquant envoyant deux blocs individuellement valides à la même
hauteur, tous deux sous la profondeur de finalité, faisait co-signer les deux par un
validateur **honnête**. Des validateurs honnêtes contribuaient donc à deux branches
concurrentes, et l'invariant

```
un validateur signe au plus un bloc par hauteur
```

n'était pas tenu. C'est exactement la précondition du scénario de double finalité
(VX-RED-004) : sans cet invariant, la sûreté BFT ne repose plus sur rien.

`détecter ≠ empêcher`. Un slashing a posteriori punit le fautif ; il ne restaure pas la
sûreté d'une chaîne qui a déjà finalisé deux branches.

## 2. Décision

Un **verrou de vote durable**, consulté **avant** toute signature.

### 2.1 Stockage

Une table redb dédiée `votes` : `height (u64) → block_hash ([u8; 32])`. C'est le seul vote
que ce validateur a émis à cette hauteur.

### 2.2 Primitive

`Storage::claim_vote(height, block_hash) -> io::Result<bool>` — un test-and-set validé avant
retour :

| État à `height` | Retour | Sens |
|---|---|---|
| Aucun | `Ok(true)`, vote enregistré | Le vote est accordé |
| Le **même** hash | `Ok(true)` | Idempotent — un bloc re-gossipé n'est pas une équivocation |
| Un hash **différent** | `Ok(false)` | Refus : signer serait une équivocation |

L'écriture est **commitée avant le retour**. C'est le point central : un verrou écrit après
coup ne peut rien empêcher, et un verrou en mémoire laisserait un redémarrage entre les deux
blocs concurrents rouvrir la faille.

### 2.3 Points d'application

- **Chemin P2P de co-signature** (`p2p/mod.rs`) : le verrou est réclamé avant `bls_sk.sign`.
  Un refus interrompt le traitement sans signer.
- **Production de bloc** (`node.rs`) : produire un bloc *est* un vote à cette hauteur —
  `produce_block` le co-signe. Le même verrou est réclamé, de sorte qu'un validateur ne peut
  pas à la fois produire un bloc et co-signer un bloc concurrent à la même hauteur. Le
  verrou est réclamé après production, le hash n'étant pas connu avant.

### 2.4 Échec fermé

Si le verrou est indisponible (erreur d'E/S), le nœud **ne signe pas**. Sans trace durable,
il ne peut pas prouver qu'il n'équivoque pas, et l'équivocation est slashable : l'inaction
coûte une récompense, la signature coûte le bond.

### 2.5 Rôle conservé de `record_signature`

`record_signature` garde sa fonction distincte : indexer les preuves d'équivocation des
**autres** validateurs, pour alimenter le slashing (ADR 0003) et l'accountability
(ADR 0030). Notre propre vote est désormais **contraint** par le verrou, plus seulement
observé.

## 3. Conséquences

- Un validateur honnête ne peut plus être **induit** à équivoquer. L'équivocation résiduelle
  suppose un opérateur qui contourne délibérément son propre nœud — c'est-à-dire un
  byzantin, ce que le slashing traite.
- La table `votes` croît d'une entrée par hauteur signée. À 12 s par bloc et 40 octets par
  entrée, ~105 Mo par siècle : négligeable, mais non purgé — un élagage sous la hauteur
  finalisée pourra être ajouté si nécessaire.
- Un nœud sans `data_dir` (mode mémoire) n'a pas de stockage : le verrou n'y est pas
  appliqué. Ce mode est réservé aux tests, jamais à un validateur exposé.

## 4. Alternatives écartées

| Option | Pourquoi écartée |
|---|---|
| Utiliser `record_signature` en verrou (mémoire) | Ne survit pas au redémarrage ; un attaquant patient rouvre la faille |
| Ne compter que sur le slashing | Punir n'est pas empêcher : deux branches finalisées sont irréversibles, le slashing ne les défait pas |
| Verrou par `(height, round)` | Le protocole n'a pas de rounds explicites aujourd'hui — à reprendre si c'est le cas (voir §5) |

## 5. Limite connue

Le verrou est **par hauteur**. Si le protocole adopte des rounds explicites, il devra
devenir par `(height, round)`, faute de quoi un validateur légitimement autorisé à voter à
un round supérieur serait bloqué. Suivi dans `audit/post-fix/FINDINGS_STATUS.md`.

## 6. Critères de validation

- [x] Premier vote accordé ; re-signature du même hash idempotente ; hash différent refusé ;
      hauteurs indépendantes — `vote_lock_prevents_equivocation_and_survives_restart`.
- [x] Le refus tient après réouverture de la base (durabilité) — même test.
- [x] `cargo test --workspace` vert.
- [ ] Banc adversarial : deux blocs concurrents envoyés à un validateur honnête réel, vérifier
      qu'une seule signature sort — **non réalisé**, nécessite le harnais multi-nœuds (ADR 0080).
