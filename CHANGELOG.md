# VinX Ledger — Changelog

Suivi rétrospectif de toutes les versions du projet.  
Format : `MAJEUR.MINEUR.CORRECTIF` — les versions `0.x.y` sont des versions de développement pré-production.

---

## [0.37.0] — ⚠️ BREAKING (consensus) — Dépôt existentiel & reaping (ADR 0026)

> Ajout d'un **solde plancher** dans la fonction de transition d'état, et **suppression** (reap) des comptes vidés. Consensus-breaking : la règle entre dans l'exécution — deux nœuds avec des paramètres différents divergeraient — donc la constante est **gravée**.

- **Dépôt existentiel gravé.** `EXISTENTIAL_DEPOSIT_ATOMS = 0,001 VinX`. Un transfert qui laisserait l'expéditeur **ou** le destinataire dans `]0, ED[` est rejeté (`CoreError::BelowExistentialDeposit`). Ferme le seul terme **non borné** du modèle de stockage (spam de comptes-poussière à coût quasi nul) : immobiliser 1 M de comptes coûte désormais **1 000 VinX** au lieu de ~0.
- **Validation avant mutation.** La règle ED est vérifiée **en amont** de toute écriture (calcul de deltas par adresse, gère `from == to` et `sponsor == to`), parce que le producteur **saute une tx échouée sans rollback** — une vérif tardive aurait laissé des mutations partielles dans le bloc (divergence).
- **Reaping.** Un compte vidé à `balance == 0` (sans `staked` ni déliaison en cours) est **retiré de l'état** et **effacé du store** (redb : nouveau canal `account_deletes` ; il ne ressuscite pas au reload). L'état peut désormais **décroître**. `circulating_supply` inchangé par un reap → invariant de masse (ADR 0004) préservé.
- **Comptes stakés exemptés.** Un compte bondé (`staked > 0`) n'est jamais poussière : exempté du plancher de solde, et non reapé tant qu'il a du stake ou une déliaison en cours.
- **`credit` inchangé** (récompenses, maturation, bounty) : ses bénéficiaires sont des validateurs ou des retours de bond `≥ 1 VinX ≫ ED`, jamais de la poussière.
- Test-tripwire constitutionnel (`test_existential_deposit_is_constitutional`) + couverture : rejet poussière, transfert d'exactement ED, balayage→reap, exemption staké, non-reap tant qu'en déliaison, non-résurrection après reload.

---

## [0.36.0] — Validation de bloc parallélisée (ADR 0015, tranche 1)

> Ajout **non-breaking** : la vérification des signatures de transactions passe du séquentiel au parallèle sur tous les chemins de validation de bloc reçu. Aucune modification du format ni du consensus — même posture de sécurité, off du chemin critique séquentiel.

- **Vérification parallèle des signatures.** À la réception d'un bloc (P2P `NewBlock`, `SyncResponse`, sync au démarrage), toutes les signatures Ed25519 des transactions sont vérifiées **en parallèle** (rayon `par_iter`) **en amont**, puis l'état est appliqué **séquentiellement** via `apply_transaction_trusted`. Le coût CPU dominant de la validation passe de 1 cœur à *N* cœurs — gain direct sur la vitesse de sync et la validation de blocs pleins.
- **Sûr par construction.** `WorldState::verify_tx_signature_pure` est **pure** (ne lit pas l'état) ; la vérification est une conjonction pass/fail **indépendante de l'ordre** → déterministe, zéro risque de divergence. **L'ordre d'exécution de l'état reste strictement séquentiel** : on parallélise la *validation*, pas l'*exécution*.
- **Exécution d'état parallèle : différée** (ADR 0015 tranche 2). Documentée et encadrée (Block-STM), avec des **critères de déclenchement** explicites (banc 3-validateurs + profilage + TPS réellement au-delà d'un cœur) — pas d'optimisation prématurée sur le terme le plus risqué du système.

---

## [0.35.0] — ⚠️ BREAKING (consensus) — Gouvernance unifiée + préavis d'upgrade en temps réel

> Lot **consensus-breaking → nouvelle genèse.** Clôt les deux derniers ADR « Cohérence & Robustesse » lourds (0006, 0007). Implémenté de bout en bout (`vinx-core`, `vinx-state`, `vinx-node`, wallet CLI, desktop-core, app desktop, console admin JS, SDK). Tests réécrits, clippy `-D warnings` & fmt verts.

### ADR 0007 — Unification des chemins de gouvernance
- **Retrait des types de tx dédiés** `AddValidator` (0x05) et `RemoveValidator` (0x06). Les changements du set de validateurs passent **uniquement** par `AdminAction` (0x08) portant un `bincode(GovernanceAction)`.
- **Sémantique stricte unique** au point de validation : bond requis, **doublon refusé** (au lieu d'un ignore silencieux), retrait du **dernier validateur refusé**.
- **Nonce consommé seulement en cas de succès** : une action de gouvernance rejetée ne brûle plus le nonce (fidèle aux anciens handlers dédiés).
- Clients alignés : wallet CLI (`add-validator`/`remove-validator` route via `AdminAction`), desktop-core (`build_add/remove_validator`), console admin JS (reconstruit `bincode(GovernanceAction)` côté navigateur). Golden vector mis à jour.

### ADR 0006 — Préavis d'upgrade en temps réel
- **Activation d'upgrade : hauteur de bloc → timestamp Unix** (secondes). Cohérent avec émission et déliaison ; la hauteur n'est pas une horloge sous cadence adaptative. Ferme le **dernier écart doc↔code**.
- Constantes `UPGRADE_NOTICE_{PATCH,MINOR,MAJOR}_SECS` (7/30/90 j) remplacent les `*_BLOCKS` ; `ScheduledUpgrade.activation_ts`, `UpgradeType::min_notice_secs()`.
- Annonce et activation mesurées contre `current_block_ts`. **Payload 14 o inchangé** (octets identiques, sémantique = timestamp) → golden vector `signing_bytes` intact.
- Clients alignés : wallet `--activation-ts`, desktop-core + dto, app desktop, console admin JS (saisie/affichage en date), SDK (`activation_ts`).

### Divers
- **ADR 0026 (Proposé)** rédigé : dépôt existentiel (anti-bloat de l'état) — motivé par la simulation de stockage (l'état est le seul terme non borné).

---

## [0.34.0] — ⚠️ BREAKING — Refonte économique v5 : fair launch

> Changement **consensus-breaking → nouvelle genèse.** Abandon du cycle *melt/forge* (0.24.0) au profit du *fair launch* décrit dans le whitepaper v4.0. Implémenté de bout en bout (`vinx-core`, `vinx-state`, `vinx-node`) ; suite de tests réécrite, clippy `-D warnings` & fmt verts. **Différé** (voir `ETAT_DU_PROJET.md` §8) : préavis d'upgrade en temps réel, cosmétique UI admin / SDK.

Abandon du cycle *melt/forge* (frais fondus dans une réserve, récompensés à des stakers passifs) au profit d'un **fair launch — émission par le travail des validateurs**.

**Le nouveau modèle**
- **Aucun pre-mine.** Genèse : 0 en circulation, 100 Md scellés dans La Fonderie. Suppression de `FOUNDER_ALLOCATION_ATOMS`. Le fondateur gagne ses VINX en faisant tourner des validateurs, comme tout le monde.
- **Émission par le travail.** La Fonderie se vide uniquement pour rémunérer la production de blocs : décroissance exponentielle, **halving tous les 8 ans** (`débit(t) = R₀·2^(−t/8 ans)`, R₀ ≈ 8,66 Md/an, intégrale = 100 Md). Créditée au producteur, **non pondérée par le bond** (égalité round-robin). Calculée sur le **temps réel** (timestamps), pas la hauteur.
- **Relais automatique.** Fonderie vidée sous un seuil de poussière → **fees-only** pour toujours, sans intervention.
- **Frais forfaitaires au producteur.** Frais = `0,0001 VINX × poids(type) × congestion(×1–3)`, indépendant du montant (fin du 0,05 % ad valorem). **100 % au validateur producteur**, plus de melt. Actions admin exemptes (poids 0).
- **Staking = bond de validateur.** Fin du staking retail et des récompenses de staking. Bond min `100 000 VINX` (gouvernable) pour rejoindre le set (genesis dispensé) ; **aucun rendement**. Déliaison **3 jours de temps réel**.
- **Slashing réparé.** `SlashEvidence` portera les deux `BlockHeader` signés ; vérification réelle des deux signatures Ed25519 (le code actuel n'en vérifie aucune — faille exploitable). Équivocation → 100 % du bond, 10 % au rapporteur, reste fondu.
- **Le temps = timestamps.** Émission, déliaison et préavis d'upgrade (7/30/90 j) passent en temps réel. Bornes de timestamp ajoutées à la validation de bloc.

**Documentation alignée** : whitepaper v4.0, README, GUIDE, GETTING_STARTED, ETAT_DU_PROJET.

---

## [0.33.0] — 2026-07-09

### Migration de données — plus de wipe au changement de schéma

`Storage::open` **migre désormais les anciennes données vers l'avant, en place**, au lieu de refuser de démarrer et d'imposer une suppression du dossier. Le jour où VinX portera de la vraie valeur, une mise à jour ne fera plus perdre la chaîne.

- **Framework de migration forward** (`migrate_forward`) : applique les étapes `v_n → v_{n+1}` en séquence dans la transaction d'ouverture, puis estampille la nouvelle version.
- **Insight d'architecture exploité** : sur disque, VinX sépare la *source de vérité* (comptes, méta world-state, blocs) des *données dérivées* (les index de tx, reconstructibles depuis la chaîne). Un bump qui n'a touché que les données dérivées (comme **v6 → v7**) se migre en supprimant les index périmés — `load()` les reconstruit depuis la chaîne. Aucune perte.
- **Gardes claires** : un schéma on-disk **plus récent** que le binaire est refusé proprement (pas de downgrade) ; une transition **sans chemin de migration connu** renvoie une erreur actionnable pointant le repli snapshot (`GET`/`POST /snapshot`), au lieu d'un wipe silencieux.
- **5 tests** : migration v6→current (index reconstruits + version bumpée), **données réelles préservées** de bout en bout (solde fondateur + chaîne + index reconstruit), version inconnue → erreur guidée, version plus récente refusée, version courante inchangée.

Les futurs bumps qui changent la disposition des comptes/méta/blocs ajouteront une étape de transformation explicite au même endroit (point d'extension documenté).

---

## [0.32.0] — 2026-07-08

### Cadence simplifiée : block time 5 s, courbe unique (retrait du « premier bloc rapide »)

Retour à une **courbe de cadence unique** et abaissement du block time.

- **Retrait de la fenêtre de courtoisie de 500 ms** (v0.31.0) : le pacing applique désormais uniformément la formule `dynamic_gap`, que le bloc soit le premier après repos ou un reliquat. Code plus simple, un seul comportement à raisonner.
- **`block_time` par défaut : 5 s** (au lieu de 3 s en pratique / 10 s selon les docs — désormais aligné partout, `config.rs` et `main.rs`).
- Le compromis retenu : un paiement isolé attend jusqu'à ~5 s (au lieu de ~0,5 s), mais on **évite le foisonnement de petits blocs** sous trafic léger continu (donc pas de gonflement abusif de la chaîne d'en-têtes). Les avantages des autres régimes sont conservés : agrégation sous activité normale, resserrement progressif à la montée en charge, dos-à-dos à saturation.
- Vérifié en réel : 1 tx isolée (config par défaut) → bloc scellé en **~5,07 s**.

---

## [0.31.0] — 2026-07-08

### « Premier bloc rapide » — latence quasi nulle pour les paiements isolés

Correction du seul point faible de la cadence à la demande : sous très faible activité, un paiement isolé attendait le gap plein (~`block_time`, 10 s) avant d'être scellé. Désormais, le **premier bloc après une période de repos** est scellé après une **fenêtre de courtoisie de 500 ms** (une 2ᵉ tx arrivant dans l'intervalle embarque avec), au lieu d'appliquer la formule.

- La formule demande-échelle (`dynamic_gap`) ne gouverne plus que les **reliquats** d'un bloc précédent et le **trafic continu** ; la sortie de repos passe par la fenêtre courte.
- Résultat : latence ~0,5 s pour un utilisateur isolé, tout en gardant l'agrégation sous charge et le dos-à-dos à saturation.
- Vérifié en réel : 1 tx isolée (config `block_time` = 10 s) → bloc scellé en **~0,6 s** (fenêtre 500 ms + granularité de mesure) au lieu de 10 s.

---

## [0.30.0] — 2026-07-08

### Cadence de bloc adaptative à la demande (continue)

Généralisation de la cadence : au lieu d'un basculement binaire (block_time plein *ou* dos à dos), **l'écart entre blocs varie en continu avec la pression du mempool**, en une seule courbe couvrant les trois régimes voulus :

- **Repos** (mempool vide) → aucun bloc.
- **Activité légère** (quelques tx) → écart ≈ `block_time` (~10 s) : les tx s'agrègent en blocs périodiques.
- **Montée en charge** → l'écart **se resserre proportionnellement** au remplissage (`gap = block_time × (1 − pending / max_block_txs)`).
- **Saturation** (≥ un bloc plein en attente) → écart **nul**, blocs dos à dos.

**Détails (`node.rs`)**
- Pacing **avant** le scellage (fonction pure `dynamic_gap`), donc les **reliquats** d'un gros drainage continuent à se vider au lieu d'être bloqués jusqu'au prochain signal.
- Garde **anti-spin** conservée : un bloc vide malgré un backlog (tx inapplicables) marque `stalled` → attente d'un nouveau signal, jamais de blocs vides en boucle.
- Garde **anti-bloc-vide** : un réveil sans travail (notify parasite / mempool déjà drainé) ne produit pas de bloc.
- Suppression du `batch_window` fixe (200 ms) : la courbe de gap l'absorbe.

**Vérifié en réel** : 8 tx, blocs de 5, `block_time` 10 s → bloc #1 (5 tx) immédiat, puis écart **4,1 s** (= `10 × (1 − 3/5)`), bloc #2 (3 tx), puis repos. Régimes saturation (dos à dos) et anti-spin re-testés. 225 tests, clippy & fmt OK.

---

## [0.29.0] — 2026-07-08

### Débit — cadence de bloc adaptative + capacités relevées

**Cadence adaptative (`node.rs`)**
- Sous charge, le producteur ne dort plus systématiquement `block_time` entre deux blocs : tant qu'**un bloc plein** est en attente **et** que le dernier bloc a **inclus des transactions**, il scelle le suivant **immédiatement** (blocs dos à dos pour vider les pics). Dès qu'il rattrape le retard, il reprend la cadence normale.
- **Anti-spin** : si un bloc sort *vide* malgré un backlog (tx inapplicables — frais trop bas, trou de nonce), le producteur se marque `stalled` et **attend un nouveau signal** au lieu de produire des blocs vides en boucle. Logique extraite en fonction pure `adaptive_cadence()` + test de vérité.
- Mesuré en réel : 40 tx (blocs de 5, `block_time` = 10 s) drainées en **8 blocs en ~44 ms** (dos à dos) au lieu de ~80 s, puis **arrêt propre** (aucun bloc vide).

**Capacités relevées (`config.rs`, `main.rs`, `mempool.rs`)**
- `max_block_txs` : **1 000 → 10 000** par bloc (réglable via `config.toml`).
- Taille du mempool : **10 000 → 100 000**, désormais **configurable** (`max_mempool_size`).

**Docs** : whitepaper aligné (cadence adaptative, capacités, TPS réaliste config-dépendant au lieu du « 4000 » incohérent avec « blocs 10 s »).

---

## [0.28.0] — 2026-07-06

### Robustesse — démarrage sans panique + fin des clés typées

**Robustesse (`storage.rs`, `main.rs`)**
- `Storage::open() -> io::Result<Self>` : l'ouverture du stockage ne **panique** plus. Un échec (dossier illisible, base corrompue) remonte proprement. `Storage::new()` reste un wrapper `expect()` pour les appels internes et les tests.
- **Mismatch de version = erreur claire et actionnable**, plus un `assert!` avec backtrace : le message pointe la **procédure de migration** (exporter via `GET /snapshot` sur l'ancien binaire → réimporter via `POST /snapshot` sur un dossier vide) au lieu de « delete the data directory ».
- `main.rs` : ouverture du stockage et création du dossier de données gérées **gracieusement** (message + `exit(1)`), fini les paniques au démarrage.

**Fin des clés typées (`node.rs`, `rpc/types.rs`, `rpc/handlers.rs`)**
Les dernières maps encore keyées par `String` (adresses bech32, hashes hex) passent aux octets bruts :
- `validator_liveness` : `HashMap<String,u64>` → `HashMap<Address,u64>`.
- `faucet_cooldowns` : `HashMap<String,Instant>` → `HashMap<Address,Instant>`.
- `suspended_validators` : `HashSet<String>` → `HashSet<Address>`.
- `receipts` : `LruCache<String,…>` → `LruCache<Hash32,…>` (le handler décode l'hex une fois).
- `ValidatorSetResponse::from_validator_set` compare des `Address` au lieu de refaire des `to_string()` par validateur à chaque bloc.
- `address_cache` reste keyé par `String` **à dessein** (c'est un cache d'entrée brute → `Address`).

Validé en réel : démarrage à froid, erreur gracieuse sur dossier invalide (exit 1, pas de panique), `/validators` (liveness typée), `/tx/{hash}/receipt` (receipts `Hash32`).

---

## [0.27.0] — 2026-07-06

### Optimisations — clés typées + ahash sur le hot-path

Fin du travail engagé par le refactor `Address = [u8; 20]` : les index et files chauds n'utilisent plus de clés `String` (adresses bech32, hashes hex) mais les **octets bruts**, hachés avec **ahash**. Zéro allocation de chaîne par insertion/lecture sur le chemin critique.

**Clés typées (`chain.rs`, `mempool.rs`)**
- `Chain::tx_index` : `HashMap<String, …>` → `AHashMap<Hash32, (u64, u32)>` (clé = hash 32 octets, plus d'encodage hex à chaque bloc).
- `Chain::account_tx_index` : `HashMap<String, Vec<String>>` → `AHashMap<Address, Vec<Hash32>>`.
- `Chain::slash_evidence` : `AHashMap<Address, AHashMap<u64, AHashSet<Hash32>>>`.
- `Mempool` : `queues`, `min_nonce`, `confirmed_nonces`, tas de priorité et éviction — tous keyés par `Address` (Copy) au lieu de `String`.
- `get_tx_by_hash`/`get_account_txs`/`account_tx_count` prennent désormais `&Hash32`/`&Address` ; les handlers RPC décodent l'entrée une fois (hex → `Hash32`, bech32 → `Address`).

**ahash (`ahash` avec feature `serde`)**
- Toutes les `HashMap`/`HashSet` chaudes du mempool et de la chaîne passent à `AHashMap`/`AHashSet` (SipHash → ahash). Le refactor des clés typées simplifie aussi le code (plus de `.clone()`/`.to_string()`).

**Stockage v7** : les index persistés changent de layout (clés octets, sérialisation bincode). Ce sont des **données dérivées** — un démarrage sur données v6 force une reconstruction depuis les blocs. Round-trip vérifié par test (`test_storage_roundtrip_preserves_typed_tx_index`) et en réel (transfert → `/tx/{hash}` → redémarrage → index rechargé depuis le disque).

---

## [0.26.0] — 2026-07-06

### Console d'administration (`/admin`) + corrections critiques

Ajout d'une **console d'admin** servie par le nœud : tableau de bord, gestion des validateurs et planification des mises à jour, le tout **signé dans le navigateur** avec la clé admin (elle ne quitte jamais le poste). Aucun changement de protocole — la page s'appuie sur les endpoints existants.

**Console admin (`vinx-node/rpc/ui.rs`, `mod.rs`)**
- Nouvelle page `GET /admin`, séparée de l'explorateur public, en **lecture seule** tant qu'une clé correspondant à l'admin on-chain n'est pas chargée.
- **Tableau de bord** : hauteur, mempool, version de protocole, La Fonderie, circulation, base fee (rafraîchi en direct).
- **Validateurs** : set actif (leader/online/suspendu), ajout/retrait, et **approbation des demandes en attente** (`/validators/pending`, via token opérateur `Authorization: Bearer`).
- **Mises à jour** : planification d'upgrade (`AnnounceUpgrade`) avec version + hauteur d'activation.
- **Maintenance** : compactage du stockage + faucet.
- Actions à **signature triviale** (Phase 1) : `AddValidator` (0x05), `RemoveValidator` (0x06), `AnnounceUpgrade` (0x04) — le plancher de frais et la rotation admin (payloads bincode) viendront en Phase 2.
- `GET /network/stats` expose désormais `admin_address` (info publique) pour que la console vérifie la clé chargée. SDK TypeScript aligné.
- Vecteur de test doré **JS ↔ Rust** (`test_governance_signing_bytes_golden_vector`) verrouillant l'équivalence des `signing_bytes` de gouvernance. Chaîne complète validée en conditions réelles (signature navigateur → `/tx/submit` → bloc → set de validateurs mis à jour).

**Corrections critiques (préexistantes, hors périmètre console)**
- **Panique au démarrage à froid** (`main.rs`) : le nœud ouvrait deux handles redb sur le même fichier (`DatabaseAlreadyOpen`) — introduit par la refonte de persistance incrémentale. Le mempool est désormais lu puis le premier handle libéré avant la construction du nœud. **Le nœud démarre de nouveau.**
- **Bannière obsolète** : affichait « Admin : 21 000 000 VINX » (ancienne supply) ; corrigée pour refléter l'allocation réelle du fondateur (1 Md), calculée depuis `FOUNDER_ALLOCATION_ATOMS`.
- **Explorateur** : table de couleurs des tx nettoyée (`Emission` supprimé, ajout de `AddValidator`/`RemoveValidator`/`AnnounceUpgrade`).

---

## [0.25.0] — 2026-07-03

### Sécurité économique — Warm-up de staking (anti *just-in-time*)

Correction d'un défaut d'équité dans la distribution des récompenses : `distribute_staking_rewards` récompensait **tout** compte staké au bloc de distribution, quelle que soit la durée de détention. Le champ `stake_since` existait mais n'était jamais lu dans le calcul.

- **Warm-up (`STAKE_WARMUP_BLOCKS = 100`, une époque complète)** : un stake ne devient éligible qu'après avoir été détenu au moins une époque. Ferme l'exploit *just-in-time* — staker au bloc 999, encaisser au 1000, déstaker au 1001 ne rapporte désormais **rien**.
- **Dénominateur = stake éligible uniquement** : les récompenses restent proportionnelles entre les comptes qui gagnent réellement l'époque ; le métal non distribué reste dans la Fonderie (invariant `circulation + Fonderie == 100 Md` préservé).
- **Ancienneté pondérée par le capital sur top-up** : ajouter au stake décale `stake_since` vers le temps de commitment pondéré (`new = s + (h − s)·add / (old + add)`), pour qu'un gros dépôt tardif n'hérite pas de l'ancienneté d'un petit stake ancien. Calcul en arithmétique *checked* avec repli conservateur (reset complet du compteur en cas d'opérandes astronomiques).

**Tests** : 4 nouveaux (exclusion JIT, borne d'éligibilité, partage du pool entre stakes éligibles, pondération du top-up). Suite complète verte, clippy & fmt propres. Pas de changement de schéma de stockage (`stake_since` déjà persisté en v6).

---

## [0.24.0] — 2026-07-02

### ⚠️ BREAKING — Refonte « La Fonderie » : tokenomics melt/forge, simplification

Cristallisation de l'identité de VinX : une monnaie propre au modèle **melt/forge**, débarrassée de tout l'échafaudage stratégique (Coffre Maturité, conditions MiCA, gel judiciaire). *On ne mint pas — on **forge**. On ne brûle pas — on **fond**.* Changement de protocole cassant → **nouvelle genèse** (schéma stockage **v6**).

**Tokenomics « Fonderie » (`vinx-core`, `vinx-state`)**
- **Supply 100 Md immuable, aucun burn.** Invariant vérifié à chaque bloc : `circulating_supply + foundry == MAX_SUPPLY`. La valeur circule à l'infini, rien n'est créé ni détruit.
- **La Fonderie** (`foundry`) remplace les 6 anciens réservoirs (`staking_pool`, `melt_pool`, `distribution_pool`, `validator_fee_pool`, `treasury`, `coffre_maturity`).
- **Melt** : 100 % des frais retournent dans la Fonderie (`melt_to_foundry`) et quittent la circulation.
- **Forge** : les récompenses de staking sont forgées depuis la Fonderie (`FORGE_RATE_BPS = 10`, soit 0,1 % de la Fonderie par distribution). Comme la forge prend une *fraction* et que les frais la refont fondre, **la Fonderie ne se vide jamais** — le cycle infini.
- **Genèse** : 1 Md (1 %) forgé au fondateur pour amorcer, 99 Md (99 %) scellés dans la Fonderie.

**Suppressions (simplification)**
- **Coffre Maturité** + conditions MiCA (`CoffreCondition`, `MarkCoffreCondition`, `UnlockCoffre`) — retirés.
- **Gel judiciaire** : types de tx `FreezeAccount`/`UnfreezeAccount`, champs `Account.frozen`/`frozen_since`, `check_auto_unfreeze` — retirés.
- **Split de frais 80/20** validateur/treasury et `ReleaseMeltToDistribution` — retirés (remplacés par le melt 100 %).
- Discriminants de tx renumérotés `0x01..0x08` (8 types restants) ; `Emission` supprimé.

**Répercussions**
- `hash_account` (feuille Merkle) et le handler `/account/:address/proof` ne hachent plus `frozen`/`frozen_since`.
- `GET /network/stats` & `/metrics` exposent `foundry` au lieu des anciens pools ; `AccountResponse` sans `frozen`.
- Wallet CLI : commandes `freeze`/`unfreeze` retirées. UI web : « La Fonderie » remplace les pools, statut de gel retiré.
- SDK TypeScript aligné (`NetworkStatsResponse.foundry`, `AccountResponse` sans `frozen`).

**Portes gardées ouvertes** : versioning de protocole (`ProtocolVersion`/`ScheduledUpgrade`), dispatch de tx modulaire, multi-validateur P2P, chain-ids — intacts pour l'évolution future (token factory, etc.).

**Tests** : modèle Fonderie (melt/forge, conservation de supply), invariant `circulation + Fonderie == 100 Md`. 215 tests Rust + 23 SDK — 0 échec. clippy `-D warnings` & fmt verts.

---

## [0.23.0] — 2026-07-01

### ⚠️ BREAKING — `Address` en 20 octets bruts (design canonique)

Refonte de la représentation d'adresse : `Address(String)` (bech32) → `Address([u8; 20])` — la charge utile brute (`SHA-256(pubkey)[..20]`), façon Ethereum/Cosmos. Le bech32 (`vinx1...`) devient un simple encodage d'affichage/transport appliqué aux frontières. **Changement cassant du protocole** (signature, hachage, formats disque & wire) — réalisé maintenant, en pré-mainnet, quand le coût de coordination est minimal.

**`vinx-crypto`**
- `Address` est désormais `Copy`, 20 octets inline, sans allocation heap ; hachage et comparaison sur 20 octets fixes.
- `as_str()` supprimé ; nouveaux `as_bytes()`, `from_bytes()`, `to_bech32()`. `Display`/`Debug` encodent en bech32 à la demande.
- serde format-aware : **JSON reste `"vinx1..."`** (API RPC & SDK inchangés), binaire (bincode) = 20 octets ; borsh (P2P) = 20 octets.

**Consensus & encodages (canoniques sur octets bruts)**
- `Transaction::signing_bytes()` : adresses `from`/`to`/`sponsor` en 20 octets (préfixes de longueur supprimés). Layout figé par un test doré (`test_signing_bytes_golden_vector`).
- `BlockHeader::hash()` et `hash_account()` (feuille Merkle) hachent les 20 octets.
- Maps clées par `Address` (`accounts` `BTreeMap<Address>`, `leaf_index`, mempool, `slash_evidence`, table redb `accounts`) — plus aucune allocation de clé string, hachage plus rapide.
- Ordre des feuilles Merkle = ordre des octets d'adresse (les racines d'état changent — nouveau genesis).
- Stockage schéma **v5** (adresses 20 octets) — les données antérieures sont rejetées au démarrage.

**Clients**
- UI web (`rpc/ui.rs`) : signature corrigée — décodage bech32 → 20 octets (`from`/`to`) + `chain_id`/`expiry`/`sponsor` désormais inclus (l'ancien signeur JS était en réalité désynchronisé du Rust depuis v0.14). Vérifié bit-à-bit contre le vecteur doré Rust via Node.
- `GET /health` expose `chain_id` (le signeur navigateur s'y aligne). Type SDK `HealthResponse` étendu.

**Tests** : vecteur doré `signing_bytes`, vecteur bech32↔JS, compat serde/bincode/borsh (JSON=bech32, binaire=20 o), `Address: Copy`. 229 tests Rust + 23 SDK — 0 échec.

---

## [0.22.0] — 2026-07-01

### Allocations & tris — comptes triés, adresses partagées

**#4 — `accounts` en `BTreeMap` (`vinx-state`)**
- `WorldState::accounts` passe de `HashMap` à `BTreeMap`. La clé étant l'adresse bech32, `values()` itère déjà dans l'ordre Merkle (tri par adresse).
- Suppression du `sort_by_key` O(n log n) dans `full_rebuild` (reconstruction du state root) **et** dans `accounts_sorted` (preuves d'inclusion Merkle, chemin RPC).
- Ordre des feuilles inchangé (`BTreeMap<String>` = tri par `address.as_str()`) → racine Merkle strictement identique.

**#2 — `Address` en `Arc<str>` au lieu de `String` (`vinx-crypto`)**
- `Address(String)` → `Address(Arc<str>)` : chaque clone (blocs, transactions, signatures, validator set…) devient un **incrément de compteur de références O(1)** au lieu d'une allocation heap. L'adresse traverse tout le pipeline en étant clonée en permanence — c'est le coût d'allocation le plus diffus du système.
- `as_str()` continue de renvoyer les octets bech32 exacts dont dépendent `signing_bytes()`, l'API JSON et le hachage des feuilles Merkle — **aucun contrat client cassé**.
- Implémentations `serde`/`borsh` manuelles : les formats sérialisés restent **strictement identiques** à ceux de `String` (JSON `"vinx1..."`, bincode et borsh = longueur + UTF-8). Vérifié par 3 tests dédiés (`test_json_is_the_bech32_string`, `test_bincode_matches_plain_string`, `test_borsh_matches_plain_string`).
- Note : le passage à une représentation `[u8; 20]` `Copy` (gain maximal) reste un **chantier protocole cassant** distinct — il modifierait `signing_bytes`, le hachage Merkle et le format JSON, exigeant une MAJ coordonnée de l'UI web + du SDK TypeScript et un bump de version protocole.

**Nettoyage**
- `cargo fmt --all` + `cargo clippy -- -D warnings` sur tout le workspace (dérive de toolchain des stables récentes) : format, closures redondantes, `&PathBuf`→`&Path`, `Chain::is_empty`, `#[allow]` ciblés (type_complexity, large_enum_variant, too_many_arguments).

225 tests — 0 échec.

---

## [0.21.0] — 2026-07-01

### Persistance incrémentale & application « trusted » (scalabilité)

**#1 — Persistance des comptes par clé (`vinx-node`, `vinx-state`)**
- Schéma de stockage **v4** : les comptes ne sont plus sérialisés dans un blob monolithique à chaque bloc. Nouvelle table redb `accounts` — une ligne `adresse → bincode(Account)` par compte.
- `WorldState` porte désormais un ensemble `persist_dirty: HashSet<String>` (`#[serde(skip)]`), distinct de `dirty_addrs` (consommé par `compute_state_root`) : il survit jusqu'à ce que `take_persist_dirty()` le draine, afin d'écrire **uniquement les comptes réellement modifiés**.
- `WorldState::serialize_meta()` sérialise tous les champs **sauf** la map des comptes (celle-ci est temporairement déplacée via `mem::take` — aucun clone — puis restaurée).
- `Storage::serialize_incremental()` → écrit meta + lignes de comptes sales ; `Storage::serialize_full()` (genesis, import snapshot) → écrit tous les comptes et purge les lignes obsolètes (`replace_accounts`).
- **Gain** : le coût par bloc passe de **O(total comptes)** à **O(comptes modifiés)**. Sur 1 M de comptes dont 200 changent par bloc, on écrit 200 lignes au lieu de re-sérialiser + re-compresser l'état entier toutes les 10 s. Débloque la scalabilité à long terme (les comptes ne sont jamais élagués, contrairement à la chaîne).
- `POST /snapshot` : import déclenche désormais un `persist_full()` immédiat (purge des comptes obsolètes de l'état remplacé).

**#3 — Application « trusted » sans re-vérification de signature (`vinx-state`, `vinx-node`)**
- `apply_transaction` refactorisé en trois helpers : `check_replay_and_ttl` (chain-id + TTL, bon marché), `verify_tx_signatures` (Ed25519, coûteux) et `dispatch_tx`.
- Nouveau `apply_transaction_trusted` : applique une transaction **sans** re-vérifier sa signature Ed25519, en conservant les gardes chain-id/TTL et toutes les vérifications d'état (nonce, solde, gel…).
- Le producteur de blocs (`produce_block`, `produce_block_inner`) utilise ce chemin pour les transactions **drainées du mempool vérifié** — la signature y a déjà été validée à l'admission. Élimine une double vérification Ed25519 (l'opération la plus chère par tx).
- Les blocs reçus d'une source **non fiable** (P2P gossip, sync) conservent la vérification complète via `apply_transaction` — la frontière de sécurité est préservée.

**Tests**
- `test_incremental_persist_writes_only_dirty_rows` : vérifie qu'un seul compte modifié produit une seule ligne écrite et que l'état survit au rechargement.
- `test_apply_trusted_skips_signature_but_enforces_state` : une signature altérée est rejetée par le chemin complet mais acceptée par le chemin trusted, le nonce restant appliqué.
- `test_apply_trusted_still_enforces_chain_id_and_ttl` : chain-id erroné et TTL expiré restent rejetés même en mode trusted.
- 222 tests — 0 échec.

---

## [0.20.0] — 2026-06-30

### Quatre optimisations RPC & consensus

**D. `POST /tx/batch` — soumission par lot avec vérification parallèle (`vinx-node`)**
- Nouvel endpoint `POST /tx/batch` acceptant un tableau de 1 à 100 transactions
- Vérification des signatures en parallèle via **rayon** (aucun verrou tenu pendant le travail CPU)
- Résultats par transaction : `{ tx_hash, accepted, error? }` — les échecs n'affectent pas les autres
- Compteurs de métriques (`tx_submitted_ok` / `tx_submitted_err`) mis à jour par lot

**C. `POST /snapshot` — import de snapshot (`vinx-node`)**
- Nouvel endpoint `POST /snapshot` (admin-only) symétrique de `GET /snapshot`
- Accepte le corps JSON produit par `GET /snapshot` et remplace l'état mondial en live
- Met à jour `validator_set` et efface les validateurs suspendus après import
- Utile pour le fast-sync de nouveaux nœuds sans rejouer tous les blocs

**E. Compaction automatique du chain store (`vinx-node`)**
- Le producteur de blocs appelle désormais `chain.compact_old_txs(BLOCK_RETENTION_COUNT)` toutes les 500 blocs automatiquement (en plus du `prune` existant toutes les 1 000 blocs)
- Libère la mémoire des données d'index des transactions trop anciennes sans intervention manuelle via `/admin/compact`

**F. Éviction de vivacité des validateurs (`vinx-node`)**
- Nouveau champ `suspended_validators: Arc<RwLock<HashSet<String>>>` dans `Node`
- Après chaque bloc, le producteur détecte les validateurs absents depuis ≥ 50 blocs consécutifs et les marque comme « suspendus » (sauf le nœud lui-même)
- Les validateurs qui reviennent (bloc reçu en P2P) sont automatiquement réhabilités
- `GET /validators` expose le champ `suspended` par validateur
- `try_backup_production` : si le leader planifié est suspendu, le délai d'activation des backups est divisé par 2 (réponse plus rapide aux leaders morts)

---


## [0.19.0] — 2026-06-30

### Incremental Merkle tree — O(log n) state root par bloc

**`IncrementalMerkleTree` — `vinx-crypto`**
- Nouvelle structure qui stocke tous les niveaux de l'arbre en mémoire (niveaux `0` = feuilles, `k` = nœuds internes, racine = `levels.last()[0]`)
- `build(leaves)` : construction initiale O(n)
- `update_leaf(idx, hash)` : mise à jour d'une feuille et propagation vers la racine en O(log n) — seuls les nœuds du chemin affecté sont recalculés
- `rebuild(leaves)` : reconstruction complète O(n) — utilisée uniquement lors de l'ajout d'un nouveau compte
- `leaves()` : expose le tableau de feuilles trié, compatible avec `merkle_proof_for`
- 6 tests unitaires couvrant : racine vide, cohérence avec `merkle_root` (n = 1..10), mise à jour ponctuelle, mise à jour exhaustive, reconstruction, compatibilité des preuves d'inclusion

**`WorldState` — `vinx-state`**
- Remplacement de `cached_root: Option<Hash32>` par trois champs `#[serde(skip)]` : `merkle_tree: IncrementalMerkleTree`, `leaf_index: HashMap<String, usize>`, `dirty_addrs: HashSet<String>` et `needs_rebuild: bool`
- `mark_dirty(addr)` : remplace `invalidate_root_cache()` dans tous les `apply_*`, `credit()`, `check_auto_unfreeze()` et `distribute_staking_rewards()` ; détecte automatiquement les nouveaux comptes et active `needs_rebuild`
- `flush_dirty()` : si `needs_rebuild` → `full_rebuild()` O(n) ; sinon → `update_leaf()` pour chaque adresse sale O(|dirty| × log n)
- `compute_state_root()` : appelle `flush_dirty()` puis retourne `merkle_tree.root()` en O(1)
- **Gain mesuré** : pour un bloc de 100 tx touchant 200 comptes sur 10 000 : O(200 × 14) ≈ 2 800 ops contre O(10 000 × 14) = 140 000 ops auparavant → ~47× plus rapide sur ce scénario

---

## [0.18.0] — 2026-06-30

### Métriques P2P câblées & persistance du mempool

**Métriques P2P — Compteurs désormais actifs**
- `p2p::start()` accepte désormais `NodeMetrics` en paramètre et le transmet à la boucle d'événements
- `p2p_blocks_recv` incrémenté à chaque bloc validé et appliqué via gossip P2P
- `p2p_tx_recv` incrémenté à chaque transaction reçue via gossip P2P
- Les métriques sont partagées sans verrou entre le nœud, le rate limiter et la couche P2P

**Persistance du mempool (optimisation #3)**
- `Storage::serialize_mempool` / `save_mempool_blob` / `load_mempool` : nouvelles méthodes pour sérialiser, compresser (zstd niveau 3) et restaurer le mempool en une transaction redb dédiée
- `Node::persist()` inclut désormais le mempool dans chaque cycle de sauvegarde sur disque, sans rallonger la durée de verrouillage (sérialisation sous verrou lecture, I/O hors verrou)
- Au démarrage (`main.rs`), les transactions persistées sont rechargées et réinjectées dans le mempool en appliquant la validation complète — les transactions expirées ou invalides sont silencieusement ignorées
- Nombre de transactions restaurées affiché dans les logs (`tracing::info!`)

---

## [0.17.0] — 2026-06-30

### Rate limiting token bucket, métriques atomiques & WebSocket enrichi

**Rate limiting — Token bucket par route**
- Remplacement du compteur à fenêtre fixe par un token bucket (burst autorisé + refill continu)
- Limites différenciées par type de route :
  - `/health`, `/metrics`, `/ws`, `/events` : exempt (aucune limite)
  - `/tx/submit` : 20 req burst, refill 20/min
  - `/faucet/*` : 5 req burst, refill 5/heure
  - Tout le reste : 100 req burst, refill 100/min
- Chaque IP dispose de buckets indépendants par classe de route
- Le nombre de requêtes rejetées est compté dans `NodeMetrics.ratelimit_hit`

**Métriques Prometheus — Compteurs atomiques temps réel**
- `NodeMetrics` : struct avec 8 champs `AtomicU64` sur le `Node`, partagée avec le `RateLimiter` via `Arc`
- `GET /metrics` lit les compteurs d'activité sans aucun verrou (lock-free) ; seules les données économiques nécessitent encore un verrou
- 8 nouvelles métriques s'ajoutent aux 8 existantes :
  - `vinx_blocks_produced_total` — blocs produits par ce nœud
  - `vinx_tx_submitted_total{status="ok"|"err"}` — tx acceptées / rejetées via RPC
  - `vinx_tx_in_block_total` — tx incluses dans les blocs produits
  - `vinx_p2p_blocks_received_total` — blocs reçus via P2P gossip
  - `vinx_p2p_tx_received_total` — transactions reçues via P2P gossip
  - `vinx_ratelimit_hit_total` — requêtes rejetées par le rate limiter
  - `vinx_last_block_timestamp_seconds` — timestamp Unix du dernier bloc produit
- Les compteurs sont incrémentés aux sites d'événement : `tick()`, `submit_tx`, P2P handlers

**WebSocket — Ping/pong keepalive & événements enrichis**
- `BlockEvent` étendu avec `base_fee_atoms`, `proposer` (adresse bech32 du validateur), `state_root` (hex)
- Handler `/ws` réécrit avec `tokio::select!` sur trois branches :
  - Réception d'un `BlockEvent` → envoi JSON enrichi
  - Timer 30 s → envoi d'un `Ping` pour détecter les connexions silencieusement fermées
  - Message entrant → gestion du `Close` et des frames Pong (ignorées)
- SSE `/events` enrichi avec les mêmes champs supplémentaires

---

## [0.16.0] — 2026-06-30

### Optimisations de performance — pruning, compression, cache & mempool

**Élagage de chaîne (block pruning)**
- Constantes `BLOCK_RETENTION_COUNT = 100 000` et `PRUNE_INTERVAL = 1 000` dans `amount.rs`
- `Chain::prune(keep_last)` : vide les listes de transactions et de signatures des blocs anciens, conserve les headers
- `Chain::compact_old_txs(keep_last)` : version légère (transactions uniquement)
- Appel automatique toutes les 1 000 blocs pendant la production

**Merkle root paresseux**
- `WorldState` porte un cache `Option<Hash32>` (`#[serde(skip)]`)
- `compute_state_root()` retourne la valeur mise en cache ; le cache est invalidé uniquement aux mutations réelles : `credit()`, `apply_transaction()`, `check_auto_unfreeze()`, `distribute_staking_rewards()`
- Élimine les recalculs O(n log n) redondants sur chaque lecture RPC

**Compression zstd**
- Tous les blobs redb (state, chain, tx-index) compressés en niveau 3 à l'écriture
- Messages P2P ≥ 512 octets compressés en niveau 1 avec un octet de flag (`0x00` = brut, `0x01` = zstd)
- Réduction estimée : 60-75 % sur l'état sérialisé, 40-60 % sur les messages gossip

**Filtre de Bloom P2P**
- `bloomfilter::Bloom<Hash32>` (taux FP 1 % à 2× capacité) pré-filtre les transactions dupliquées dans `stage()`
- Évite la vérification de signature coûteuse sur les tx déjà connues
- Correctness garantie par le `HashSet<Hash32>` exact (`seen`)

**Persistance de l'index de transactions**
- `tx_index` et `account_tx_index` sérialisés avec l'état à chaque cycle (schéma v3)
- Restauration en O(1) via `import_tx_indexes()` au démarrage ; `rebuild_tx_index()` O(n) en fallback

**Persistance asynchrone**
- Sérialisation sous verrou lecture, I/O disque entièrement hors verrou via `tokio::task::spawn_blocking`
- `Storage::serialize()` (pur, sans I/O) + `Storage::save_serialized()` (I/O seul, thread-safe)
- `Storage` devient `Clone` via `Arc<Database>`

**Vérification de signatures P2P en parallèle**
- Handlers `NewBlock` et `SyncResponse` vérifient toutes les signatures de validateurs via `rayon::par_iter()`
- Chaque worker vérifie `Address::from_public_key == validator` puis la signature Ed25519
- Débit proportionnel au nombre de cœurs CPU

**Cache LRU**
- `lru::LruCache<String, Address>` (1 024 entrées) pour le décodage bech32 — `Node::parse_address()`
- `lru::LruCache<String, TxReceipt>` (100 000 entrées) pour les reçus de transaction — handler `GET /tx/:hash`

**Validation de nonce dans le mempool**
- `min_nonce: HashMap<String, u64>` : nonce minimum acceptable par émetteur, mis à jour après chaque bloc via `update_confirmed_nonces()`
- `add()` rejette immédiatement les nonces périmés (`MempoolError::StaleNonce`) avant toute vérification de signature
- Remplacement au même nonce autorisé uniquement si le frais est strictement supérieur (fee-bump)
- `MempoolError::NonceTaken` si le slot est occupé avec un frais égal ou supérieur
- `update_confirmed_nonces()` purge également les entrées périmées de la queue et reconstruit le filtre de Bloom

**Éviction par frais**
- `try_evict_for()` : à capacité maximale, une tx à frais élevé peut évincer la tx de plus faible frais dans toutes les queues
- Tx avec frais égal ou inférieur rejetée avec `MempoolError::Full`

---

## [0.15.0] — 2026-06-29

### Block-on-demand, heartbeat conditionnel & Δ1-Δ4

**Block-on-demand**
- Le producteur de blocs ne tourne plus sur un intervalle fixe de 3 s
- `tokio::select!` entre `tx_ready.notified()` (nouvelle tx admise) et `sleep(heartbeat)`
- Fenêtre de batch 200 ms après le signal tx pour regrouper les soumissions simultanées
- `Mempool::tx_ready: Arc<Notify>` — signal déclenché à chaque `add()` et `flush_staged()`
- Constantes : `HEARTBEAT_INTERVAL_SECS = 3_600`, `BATCH_WINDOW_MS = 200`
- Économie estimée ~90 % d'espace disque en période creuse (24 blocs/jour au lieu de 28 800)

**Heartbeat conditionnel**
- Heartbeat horaire ignoré si le mempool est vide et qu'aucun timer protocolaire n'est actif
- Zéro bloc vide produit en période d'inactivité totale

**Δ1 — Répartition des frais 80/20**
- Nouveau modèle : 80 % pour le validateur (`VALIDATOR_FEE_BPS = 8000`), 20 % vers le treasury
- Champ `treasury` ajouté dans `WorldState`
- `treasury_balance()` : accesseur dédié
- Remplacement de la répartition tripartite (staking/validateur/melt) par le modèle 80/20

**Δ2 — Transactions sponsorisées (gasless UX)**
- Champs optionnels `sponsor`, `sponsor_pub_key`, `sponsor_signature` dans `Transaction`
- `with_sponsor(keypair)` : builder method
- `signing_bytes()` intègre l'adresse sponsor pour le binding cryptographique
- `apply_transfer()` : montant débité au sender, frais débités au sponsor si présent
- Validation de la clé/adresse/signature sponsor dans `apply_transaction()`

**Δ3 — Format wire Borsh pour P2P**
- `BorshSerialize` / `BorshDeserialize` sur : `Address`, `PublicKey`, `VinxSignature`, `Amount`, `Transaction`, `Block`, `BlockHeader`, `BlockSignature`, `P2pMessage`
- `P2pMessage::encode()` / `decode()` basculent de bincode vers Borsh

**Δ4 — Transport QUIC**
- Feature `quic` activée sur libp2p dans `vinx-node`
- Le Swarm écoute sur un port QUIC (UDP) en plus du port TCP

---

## [0.14.0] — 2026-06-24

### Robustesse protocole (A-E)

**A — TTL & replay protection**
- Champ `expires_at_height` dans `Transaction` : TTL par hauteur de bloc
- Champ `chain_id` dans `Transaction` : protection contre le replay cross-réseau
- `signing_bytes()` intègre `chain_id` (4 B BE) et `expires_at_height` (8 B BE)
- Builders : `with_chain_id()`, `with_expiry()`
- `receipts_root` dans `BlockHeader` : SHA-256 des hashes des tx incluses

**B — Persistence redb**
- Remplacement de bincode + fichiers plats par redb (KV embarqué ACID en Rust pur)
- 2 tables : `state` (world_state, chain) et `meta` (schema_version)
- Garde de version de schéma : données obsolètes rejetées au démarrage

**C — Chain IDs**
- Module `chain_id.rs` : `MAINNET = 1`, `TESTNET = 7`, `DEVNET = 42`
- `WorldState.chain_id` avec default serde
- `GenesisConfig.chain_id` câblé à la genèse
- Flag CLI `--chain-id` dans `vinx-node`

**D — Bootstrap P2P & réputation des pairs**
- `bootstrap_peers` dans `NodeConfig` : dialés au démarrage P2P
- `peer_reputation: HashMap<PeerId, i32>` dans la boucle d'événements
- Messages gossip invalides décrémentent le score ; bannissement à -5 via `blacklist_peer`
- Flag CLI `--bootstrap-peers`

**E — Expérience développeur**
- `GET /tx/estimate` : `base_fee`, `recommended_fee`, pression mempool
- `GET /tx/:hash/receipt` : `TxReceipt` avec succès/erreur par tx
- `TxReceipt` stocké dans `Node.receipts` à chaque tick
- `Mempool::prune_expired(height)` : supprime les tx expirées à chaque tick
- Rate limiting par adresse dans le mempool : 50 tx pendantes max par émetteur

---

## [0.13.0] — 2026-06-17

### Validateur set dynamique

**Slot skip**
- `ValidatorSet::index_of()` + `leader_idx_at()` pour le calcul du leader de secours
- `validate_block()` assoupli : tout validateur enregistré peut proposer (slot skip sans violation PoA)
- `produce_block_backup()` dans `producer.rs`
- `Node::tick_as_backup()` + `try_backup_production()` : activation échelonnée (backup à distance 1 après 2×block_time, distance 2 après 3×block_time, etc.)

**Liveness tracking**
- `Node::last_block_instant` : horloge réinitialisée à chaque bloc produit
- `Node::validator_liveness: HashMap<addr, last_height>` : mis à jour dans `after_block_produced()`
- `GET /validators` enrichi : statut online/offline, `last_seen_height`, `is_next_leader`, `next_leader`

**Inscription à la validation**
- `Node::validator_requests` : queue en mémoire des demandes d'adhésion
- `POST /validators/request` : soumettre une demande (adresse + multiaddr)
- `GET /validators/pending` : liste admin-only des demandes en attente

**Tests**
- 3 nouveaux tests : `test_validator_join_request`, `test_validator_liveness_tracking`, `test_add_validator_updates_set`
- 212 tests — 0 échec

---

## [0.12.0] — 2026-06-13

### Faucet, TLS, light client Merkle, Grafana & pipeline de release

**Faucet testnet**
- `POST /faucet/request` : envoi automatique de VinX à une adresse, cooldown 24 h par adresse
- Configuration via `config.toml` : montant, cooldown, clé faucet
- Test d'intégration `test_faucet_endpoint`

**Infrastructure TLS**
- Reverse proxy Caddy : certificat auto-signé local (devnet), Let's Encrypt automatique (VPS)
- Gestion CORS, keep-alive SSE/WebSocket, redirection HTTP → HTTPS

**Light client Merkle**
- `verifyMerkleProof(leafHash, proof, stateRoot)` côté navigateur via `crypto.subtle` (SHA-256 natif)
- Intégré dans le SDK TypeScript et l'interface web

**Tests de robustesse**
- Test `test_crash_recovery` : redémarrage du nœud + vérification de la continuité de l'état
- 11 invariants proptest : conservation des frais, déterminisme Merkle, conservation de la supply, incrément de nonce sur transfert, rejet nonce incorrect

**Observabilité**
- Stack Grafana/Prometheus : dashboard auto-provisionné, scrape 15 s
- Panneaux : hauteur de chaîne, taille mempool, base fee, soldes des pools dans le temps

**Pipeline de release**
- `.github/workflows/release.yml` : tag `vX.Y.Z` → build, tests, création de release GitHub
- Guide de démarrage rapide `GETTING_STARTED.md`
- 207 tests — 0 échec

---

## [0.11.0] — 2026-06-12

### Gouvernance admin-only & distribution pool

Refonte fondamentale du modèle de gouvernance suite à la décision de design : VinX Labs conserve le contrôle exclusif du protocole on-chain. Suppression complète du système de vote entre validateurs.

**Gouvernance**
- Suppression du système de propositions et de vote on-chain (`SubmitProposal` 0x0B, `VoteProposal` 0x0C)
- Remplacement par `AdminAction` (0x0B) : encapsule une `GovernanceAction`, requiert la signature admin, s'exécute immédiatement
- Les propositions et sondages communautaires se font hors-chaîne (Discord, forum)

**Pool de distribution**
- Renommage `ReleaseMeltToStaking` → `ReleaseMeltToDistribution`
- Ajout du champ `distribution_pool` dans `WorldState` : réceptacle du melt redistribué et du Coffre Maturité débloqué
- `UnlockCoffre` libère vers `distribution_pool` (pas `staking_pool`)
- Le melt n'est pas un burn : les tokens restent en circulation dans le pool de distribution

**Corrections supply**
- Rétablissement des constantes correctes : 21 millions VinX (admin) + ~99,979 milliards (Coffre Maturité) = 100 milliards max
- Correction de `storage_test.rs` qui contenait une valeur erronée (21 milliards au lieu de 21 millions)

**SDK TypeScript**
- Suppression des types `ProposalResponse`, `ProposalListResponse` et des méthodes `proposals()` / `proposal()` (routes `/governance/proposals` supprimées)
- Ajout de `distribution_pool` dans `NetworkStatsResponse`

**Infrastructure**
- Fusion de la branche `claude/awesome-gauss-IMII2` dans `main` (13 commits)
- Suppression des doublons de tests introduits par le merge (`make_block`, tests Merkle)
- 42 tests Rust + 17 tests SDK Jest — zéro échec

---

## [0.10.0] — 2026-06-12

### WebSocket, rotation admin & Coffre Maturité

**Temps réel**
- Endpoint WebSocket `/ws` : push des événements de bloc en temps réel (complément des SSE `/events`)
- Gestion des connexions multiples simultanées

**Sécurité admin**
- Auth token Bearer sur `/snapshot` et `/admin/compact` (champ `admin_token` dans `NodeConfig`)
- Rotation de la clé admin via `AdminAction::RotateAdmin` sans redémarrage du nœud

**Coffre Maturité**
- Ajout des 3 conditions d'unlock : `MicaCasp`, `ExternalAudit`, `PublicPolicy`
- `GovernanceAction::MarkCoffreCondition` : marque une condition comme remplie
- `GovernanceAction::UnlockCoffre` : refuse si les 3 conditions ne sont pas toutes vraies
- Champs `coffre_mica_casp`, `coffre_external_audit`, `coffre_public_policy` dans `WorldState`

**Tests**
- Test d'intégration multi-validateur : `test_admin_action_adds_validator`

---

## [0.9.0] — 2026-06-12

### Frais dynamiques, slashing, HD wallet, SDK TypeScript & CI

**Économie & frais**
- Frais dynamiques style EIP-1559 : multiplicateur ×1 à ×3 selon la charge du mempool (>80% → surge)
- Répartition des frais 40 % staking / 30 % validateur / 30 % melt pool
- Fee floor configurable, `base_fee` recalculé à chaque bloc
- File d'attente mempool triée par priorité de frais (highest-fee-first)

**Sécurité réseau**
- Slashing : preuve d'équivocation (`SlashEvidence`) → 10 % bounty au rapporteur, 90 % vers melt pool, validateur exclu du set
- Vérification des signatures en parallèle via `rayon` (tous les cœurs CPU disponibles)
- Pipeline de staging parallèle pour la validation des tx en mempool
- Rate limiting HTTP : 100 requêtes/minute par IP (middleware Axum `ConnectInfo`)

**Cryptographie**
- HD wallet BIP-39 : mnémonique 12 mots → seed → clé Ed25519
- Preuves Merkle d'inclusion : `merkle_proof_for`, `verify_merkle_proof`, `MerkleProofStep`
- Endpoint `/account/:address/proof` exposant la preuve en JSON

**Réseau P2P**
- mDNS : découverte automatique des pairs en LAN sans configuration
- Validation cryptographique des blocs avant application P2P
- Détection d'équivocation sur les blocs reçus par P2P

**SDK TypeScript** (`sdk/vinx-sdk/`)
- Package npm complet avec TypeScript + Jest
- Client `VinxClient` : 13 méthodes typées couvrant tous les endpoints
- 17 tests Jest couvrant tous les appels RPC
- Types : `AccountResponse`, `BlockResponse`, `TxResponse`, `ValidatorSetResponse`, `NetworkStatsResponse`, etc.

**CI/CD**
- GitHub Actions : 4 jobs (`test`, `clippy`, `fmt`, `sdk-test`)
- Script `scripts/devnet.sh` : lance un devnet 3 nœuds local avec flag `--clean`

---

## [0.8.0] — 2026-06-12

### Phase testnet : sync, Docker, métriques & SSE

**Synchronisation**
- Ordre strict des transactions par nonce dans le mempool (rejection si nonce incorrect)
- Synchronisation de blocs au démarrage depuis un pair de confiance (`--sync-peer` HTTP)
- Endpoint GET `/chain/sync?from=N&limit=L` pour la synchronisation légère
- Historique des transactions par compte (`/account/:address/txs`) avec pagination

**Infrastructure**
- `docker-compose.yml` : 3 nœuds + volumes persistants
- Script `scripts/setup-testnet.sh` pour initialiser un testnet multi-machine
- Compaction de chaîne : `POST /admin/compact` supprime les tx anciennes, conserve les headers
- Endpoint `GET /snapshot` : export JSON complet de l'état (state + tip hash)

**Observabilité**
- Métriques Prometheus sur `GET /metrics` : hauteur, supply, staking pool, melt pool, transactions
- Endpoint `GET /network/stats` : base_fee, pools économiques, supply circulante

**Temps réel**
- Server-Sent Events `GET /events` : push de `{"type":"new_block","height":N}` à chaque bloc
- Reconnexion automatique côté client

**Validateurs dynamiques**
- `GovernanceAction::AddValidator` / `RemoveValidator` : modifie le set actif sans redémarrage
- Vérification complète des blocs P2P avant application (hash, signatures, quorum)

---

## [0.7.0] — 2026-06-11

### P2P gossipsub, gouvernance admin & versioning protocole

**P2P libp2p**
- Gossipsub pour la propagation des blocs et transactions entre pairs
- Configuration via `--p2p-listen` et `--peers` (CLI + config TOML)
- Module `vinx-node/src/p2p.rs` intégré dans la boucle tokio

**Administration**
- Transactions `FreezeAccount` (0x04) et `UnfreezeAccount` (0x05) : gel judiciaire de compte
- Gel automatique après 365 jours × 10 s/bloc (3 153 600 blocs)
- Vérification que l'émetteur est bien la clé admin dans `WorldState`

**Versioning protocole**
- `ProtocolVersion { major, minor, patch }` dans `WorldState`
- Transaction `AnnounceUpgrade` (0x06) : annonce avec délai minimal obligatoire selon le type (patch/minor/major)
- `ScheduledUpgrade` : activation automatique à la hauteur annoncée

**RPC & CLI**
- Lookup de transaction par hash : `GET /tx/:hash`
- Wallet : commandes admin (`freeze`, `unfreeze`, `announce-upgrade`, `admin-action`)
- Configuration TOML `config.toml` : tous les paramètres CLI disponibles en fichier
- UI web enrichie : affichage de l'état de gel, versions protocole, validateurs

---

## [0.6.0] — 2026-06-10

### PoA Threshold & Merkle state root

**Consensus PoA Threshold**
- Quorum ⌈2n/3⌉ : un bloc est finalisé quand suffisamment de validateurs l'ont co-signé
- Round-robin leader : chaque validateur produit à son tour selon `height % n`
- Finalité déterministe et immédiate (pas de réorganisation)
- `ValidatorSet` : liste des validateurs actifs, calcul du quorum, vérification des signatures

**Merkle state root**
- `compute_state_root()` : hash Merkle de tous les comptes (triés par adresse)
- `hash_account()` : sérialisation canonique d'un compte en bytes
- Inclus dans chaque `BlockHeader` (`state_root: Hash32`)
- Preuves d'inclusion vérifiables par les clients légers

**Persistence**
- `Storage::new(path)` / `save(&state, &chain)` / `load()` : sérialisation bincode sur disque
- Reprise automatique au redémarrage depuis le dernier état persisté
- Test `test_state_persists_across_node_restarts` : valide la continuité sur 5 blocs

---

## [0.5.0] — 2026-06-09

### Tokenomics refactor & whitepaper v2.4

**Tokenomics**
- Supply totale fixée à 100 milliards de VinX (précision 18 décimales)
- Constantes immuables : `MAX_SUPPLY_ATOMS`, `ADMIN_ALLOCATION_ATOMS`, `COFFRE_MATURITY_ATOMS`
- Fee floor par défaut : 0,0001 VinX ; constantes `FEE_NUMERATOR` / `FEE_DENOMINATOR` (0,05 %)
- Staking rewards distribués toutes les `STAKING_DISTRIBUTION_INTERVAL` blocs (100 blocs)
- Stake minimum : 1 VinX (`MIN_STAKE_ATOMS`)

**Documentation**
- Whitepaper v2.3 : décision de stack custom Rust (abandon de Substrate)
- Whitepaper v2.4 : passage de FBA à PoA Threshold comme algorithme de consensus
- README mis à jour avec la vue d'ensemble du projet

---

## [0.4.0] — 2026-06-03

### Interface web & récompenses de staking

**Interface web**
- Dashboard HTML/CSS/JS embarqué dans le binaire Rust (`rpc/ui.rs`)
- Explorateur de blocs : navigation prev/next/latest, liste des transactions
- Lookup de compte : solde, nonce, statut
- Envoi de transactions depuis le navigateur (signature Ed25519 via TweetNaCl — clé privée jamais transmise au serveur)
- Tabs Transfer / Stake / Unstake avec estimation de frais
- Affichage des validateurs et du quorum
- Mises à jour temps réel via SSE
- Thème sombre, responsive

**Staking**
- Distribution proportionnelle des récompenses à tous les stakers à chaque bloc
- Vérification du stake minimum

**Correctifs**
- Compatibilité Windows pour le wallet CLI (séparateur `--` obligatoire)
- Affichage d'un message clair si un compte n'existe pas (404 → message UI)

---

## [0.3.0] — 2026-05-25

### Implémentation initiale complète — 114 tests

Premier état complet et fonctionnel du projet. Toutes les briques de base opérationnelles de bout en bout.

**Protocole**
- 7 types de transactions : `Transfer`, `Stake`, `Unstake`, `FreezeAccount`, `UnfreezeAccount`, `Emission`, `AddValidator`
- `WorldState` : gestion des comptes, balances, nonces, staking
- Validation complète des transactions (signature Ed25519, nonce, solde suffisant)
- `Chain` : séquence de blocs avec index par hauteur, hash tip
- Genesis state configurable (`create_genesis_state`, `GenesisConfig`)

**Réseau**
- Nœud HTTP RPC (`vinx-node`) avec Axum
- Endpoints : `/health`, `/chain/height`, `/account/:address`, `/block/:height`, `/mempool/size`, `/validators`, `/tx/submit`
- Mempool avec validation et déduplication
- Producteur de blocs asynchrone (tokio), intervalle configurable

**Wallet CLI**
- `keygen` : génération de paire de clés Ed25519 + adresse Bech32 `vinx1...`
- `balance` : consultation du solde
- `transfer` : envoi de VinX signé
- `stake` / `unstake` : gestion du staking
- `block` : consultation d'un bloc par hauteur

**Tests**
- 114 tests unitaires et d'intégration couvrant : crypto, transactions, state, chain, RPC HTTP

**Documentation**
- Guide de démarrage pas à pas

---

## [0.2.0] — 2026-05-23

### Workspace Rust & crates fondamentaux

Mise en place de l'architecture multi-crates.

**`vinx-crypto`**
- Ed25519 (dalek) : `KeyPair`, `PublicKey`, `Signature`
- SHA-256 : `sha256()`
- Arbre de Merkle : `merkle_root()`
- Adresses Bech32 `vinx1...` : encodage/décodage
- `Hash32` : alias `[u8; 32]`

**`vinx-core`**
- `Amount` : entier u128 en atomes (10⁻¹⁸ VinX), arithmetic checked
- `Transaction` : structure signée avec `signing_bytes()` et `hash()`
- `Block` / `BlockHeader` : avec hash déterministe
- `Account` : balance, nonce, staked, frozen
- `CoreError` : erreurs typées
- `ValidatorSet` : placeholder initial

**`vinx-state`**
- `WorldState` : état mutable du registre
- `apply_transaction()` : dispatch par type de tx
- Tests unitaires sur chaque type de transaction

---

## [0.1.0] — 2026-05-21

### Cadrage & initialisation

- Initialisation du dépôt GitHub `HaitoDann/VinX-Ledger`
- Document de cadrage : définition du projet VinX Ledger (blockchain L1 de paiement)
- Choix technologiques initiaux : Rust, Ed25519, Bech32, PoA
- `.gitignore` pour les artefacts Rust

---

*Ce fichier est maintenu manuellement à chaque sprint. Pour voir le détail d'un commit : `git show <hash>`.*
