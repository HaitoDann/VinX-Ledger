# Décisions d'architecture (ADR) — VinX Ledger

Ce dossier trace les décisions d'architecture. Chaque ADR a un **statut** :
`Accepté`, `Proposé` (en attente de décision), `Rejeté`, `Remplacé`.

## Acceptés

| # | Titre | Statut |
|---|-------|--------|
| [0001](./0001-l1-monnaie-pure-modules-ancrage-bonde.md) | L1 monnaie pure + surcouches par ancrage bondé | Accepté (design, non implémenté) |

---

## Backlog proposé (améliorations de VinX)

Catalogue priorisé des ADR à écrire/décider. **Priorités :** 🔴 haute · 🟠 moyenne · 🟢 future.
Rien ci-dessous n'est décidé — ce sont des propositions à instruire une par une.

> **Lot « Cohérence & Robustesse » — ✅ tout implémenté** (documents dédiés) :
> [0005](./0005-temps-reseau-robuste.md) · [0006](./0006-preavis-upgrade-temps-reel.md) ·
> [0007](./0007-unification-gouvernance.md) · [0008](./0008-chain-id-defaut-sur.md) ·
> [0009](./0009-frais-stake-unstake.md) · [0020](./0020-serialisation-canonique.md) ·
> [0021](./0021-immutabilite-emission.md).
> **Anti-bloat de l'état :** [0026](./0026-depot-existentiel.md) (✅ implémenté).

### Consensus & finalité

- **0002 — Finalité au quorum** 🔴 **— ✅ tranche 1 implémentée**
  *Problème :* le producteur commite le bloc avec sa **seule** signature ; le quorum n'est vérifié qu'*a posteriori*.
  *Fait :* pointeur `finalized_height` explicite et **prefix-closed** (avance sur le préfixe contigu de blocs quorum-signés), mis à jour à la production et à chaque co-signature, exposé sur `/health`, avec `is_final(height)`. À n=1 la finalité est immédiate ; à n≥2 elle suit les co-signatures. **+ Refus de bâtir dans le vide** : le producteur (leader **et** backup) refuse de sceller au-delà de `MAX_UNFINALIZED_DEPTH` (64) blocs non finalisés au-dessus de la finalité → borne les forks concurrents et la fenêtre du fork-choice (0031). À n=1 jamais déclenché ; mord seulement si la finalité est réellement bloquée. Testé.
  **+ Banc multi-process réel** (`scripts/bench-n3.sh`) : amorce n=3 (genèse partagée déterministe + pré-financement dev), et **valide en réel** liveness (finalité en lockstep), tolérance à 1 panne (2/3 finalise), et sûreté (finalité gelée à 1/3). A **révélé et corrigé** un bug : gossip **et** sync rejetaient les blocs **backup** (proposeur ≠ leader strict) → finalité figée dès n≥2 ; désormais tout validateur enregistré peut proposer, aligné sur `consensus::validate_block`.
  **+ Fait — quorum historique (✅ corrigé, validé au banc) :** `advance_finality` évalue chaque bloc contre le **quorum du set (complet) bondé à sa hauteur** (schedule `(from_height, quorum)` reconstruit déterministiquement à l'application, `Chain::note_quorum`/`quorum_at`), et non contre le seul quorum courant. Corrige le blocage du préfixe après un **changement de set par gouvernance** (banc : les 3 nœuds finalisent en lockstep 7→10). `advance_finality` aussi rappelé sur les chemins de sync (P2P + HTTP). **Note (ADR 0027 t2b) :** ce quorum-par-hauteur suit les changements de **taille du set** (gouvernance), **pas** le jailing — le jailing ne réduit jamais le seuil de finalité (sûreté sous partition).
  *Reste (nécessite le banc 3-validateurs) :* view-change formel, latence de finalité = 1 aller-retour ; persistance du schedule quorum (aujourd'hui reconstruit à l'application → après reload, repli sur le quorum courant pour le petit suffixe non finalisé).

- **0027 — [Fiabilité & jailing des validateurs](./0027-fiabilite-jailing-validateurs.md)** 🟠 **— ✅ t1 (cœur pur) + t2a (état câblé) + t2b (rotation, validé au banc n=3) ; Unjail & règle 2 à venir**
  *Problème :* aucun mécanisme pour écarter un validateur lent/hors-ligne (distinct du slashing d'équivocation) ; un leader mort dégrade la liveness à chaque tour.
  *Fait (t1) :* `vinx-core::reliability` — fonctions **pures et déterministes** (attribution des manquements, jailing au seuil, set actif, quorum ajusté, unjail, plancher de liveness ; 5 tests). *Fait (t2a) :* **état câblé dans la transition** — champ `WorldState.reliability` (dernier champ sérialisé), **migration meta v10→v11** (append map vide, testée), mis à jour déterministiquement dans `settle_block` (hook universel : production/backup/P2P/sync) avec le proposeur effectif → table identique sur tous les nœuds. *Fait (t2b) :* la **rotation** leader (`produce_block`) et la file de **backup** (`try_backup_production`) opèrent sur le **set actif** (`active_leader_at`/`active_validators`, jailés sautés). **⚠️ Correction de sûreté (révélée par le banc n=3) :** le **quorum de finalité reste sur le set complet bondé** (`⌈2n/3⌉`), **jamais** réduit par le jailing — la table `reliability` est dérivée/subjective sous partition ; la réduire laisserait une minorité (split 1│2) jailer la majorité dans sa vue, tomber à quorum 1 et finaliser une branche rivale (double-finalité). Seule la gouvernance (`RemoveValidator`, committée) réduit `n`. Banc : à 1/3 vivant la finalité **gèle**, à 2/3 elle avance. *Reste :* tx `Unjail` (opérateur, après cooldown), règle 2 (co-signatures absentes).

- **0029 — [Décentralisation à grande échelle : agrégation & comité dynamique](./0029-agregation-signatures-comite-dynamique.md)** 🟢 **— Proposé** (évolution majeure du consensus)
  *Problème :* le round-robin + *tous* co-signent est O(N) → plafonne à des dizaines de validateurs.
  *Direction (phasée) :* **phase 1** agrégation **BLS** (N signatures → 1 agrégat + bitmap, blocs bornés, centaines de validateurs) ; **phase 2** **échantillonnage de comité par VRF** (tirage uniforme parmi les bondés, leader imprévisible anti-DoS, milliers de validateurs). Étend (ne remplace pas) le quorum ; intègre 0027/0028 ; exige 0014 (weak subjectivity) avant la phase 2.

- **0030 — [Accountability des co-signatures conflictuelles](./0030-accountability-cosignatures-conflictuelles.md)** 🔴 **— Proposé** (sûreté)
  *Problème :* la primitive de slashing on-chain punit **déjà** tout validateur signant deux en-têtes conflictuels à la même hauteur, mais le P2P ne **détecte** que la double-*proposition*, pas les **co-signatures** conflictuelles → la finalité n'est pas *accountable* (une double-finalité resterait impunie).
  *Direction :* fermer la boucle **côté détection** (rétention bornée des en-têtes/co-sigs conflictuels, assemblage de `SlashEvidence`, auto-report) — la vérif on-chain est inchangée. Rend la finalité (0002) économiquement *accountable* (argument de recoupement de quorums). Coût faible, sûreté élevée.

- **0031 — [Règle de fork-choice](./0031-regle-fork-choice.md)** 🔴 **— ✅ t1 (fonction pure) + t2a (fondation `Chain`, observation) ; wiring réorg vivant (t2b) à venir**
  *Problème :* on a `finalized_height` (plancher inréorganisable) mais **aucune règle déterministe** pour choisir entre forks concurrents *avant* finalité — un leader et son backup peuvent produire deux blocs valides à la même hauteur ; le « premier-vu » actuel dépend de l'ordre réseau (divergence transitoire).
  *Fait (t1) :* `consensus::canonical_head` — fonction **pure et totale** (poids de co-signatures → leader prévu → plus petit hash), élection **indépendante de l'ordre**, 6 tests. *Fait (t2a) :* **fondation dans `Chain`** — store de candidats concurrents (`record_candidate`, non persisté), `canonical_choice`/`would_reorg_at` (décision pure), purge sous la finalité ; **observation seulement, aucune réorg** — le store n'est pas encore alimenté par le chemin d'acceptation. 4 tests. *Reste (t2b) :* alimenter le store depuis le gossip/sync puis **réorganiser** (reconstruction d'état + réorg bornée sous finalité), à éprouver au banc multi-nœuds. Cohérente avec le slot-skip (0027) et la VRF future (0029) ; évite en amont la double-finalité que 0030 punit en aval.

- **0032 — [Garde-fous de gouvernance](./0032-garde-fous-gouvernance.md)** 🟠 **— 🚧 brouillon de discussion**
  *Problème :* depuis 0011, un comité capté/piraté peut bricoler la chaîne (frais censurants, vidage du set, verrouillage de la gouvernance) — rien ne borne l'amplitude de ces pouvoirs.
  *Direction (à trancher ensemble) :* bornes dures **uniquement sur l'irréversible** (verrouillage de gouvernance, plancher du set), bornes de *vitesse* sur le réversible (frais) ; arbitrage central **immuable vs gouvernable**. Tension à ne pas rater : résistance à la capture ⟂ réactivité en urgence (0017). Questions ouvertes listées, aucune valeur gravée.

- **0036 — [Bornes de churn du set de validateurs](./0036-bornes-churn-validateurs.md)** 🟠 **— Proposé**
  *Problème :* 0009 borne les unbonds *par compte* mais pas l'**agrégat** — une sortie massive simultanée (même honnête) fait chuter le set d'un coup → quorum inatteignable / sécurité effondrée.
  *Direction :* **file de sortie (et d'entrée) bornée** — au plus N sorties par fenêtre, ordre déterministe, bond retenu/slashable dans la file, **plancher de set** jamais franchi (partagé avec 0032). Généralise le délai d'unbonding (0009) au set entier ; distinct du jail temporaire (0027) ; s'articule avec l'admission permissionless (0033).

- **0005 — Temps réseau robuste** 🟠 **— ✅ implémenté** (bornes de timestamp à la réception — P2P **et** sync HTTP — et horloge protocole sur le Median Time Past : émission, déliaison et activation d'upgrade comparent au MTP incluant le bloc appliqué, sur les trois chemins production/P2P/sync ; à éprouver au banc n≥2)
  *Problème :* l'émission et la déliaison font confiance au `timestamp` du bloc, posé par un seul producteur. Bornes actuelles : monotonie + horloge locale à la production seulement.
  *Direction :* timestamp = médiane des horloges des validateurs (façon Bitcoin "median time past"), bornes strictes à la **réception** P2P (plafond futur, monotonie).
  *Compromis :* nécessite d'échanger/valider des horloges — pertinent surtout à n≥2.

### Sécurité

- **0003 — Slashing automatique de l'équivocation** 🔴 **— ✅ implémenté**
  *Fait :* sur réception d'un second bloc différent du même proposeur à une hauteur déjà scellée (double-proposition), on assemble l'`SlashEvidence` à deux en-têtes et on construit/soumet/gossipe automatiquement une tx `SlashValidator` (rapporteur = validateur local). Preuve cryptographiquement vérifiée → pas de faux positifs.
  *Reste :* détection via co-signatures conflictuelles (nécessite de conserver les deux en-têtes signés).

- **0012 — Gestion des clés validateur** 🟢
  *Problème :* la clé validateur est « chaude » dans le process du nœud (elle signe blocs + co-signatures + dérive la clé P2P).
  *Direction :* séparation *signer* (remote signer / HSM), rotation de clé validateur, clé P2P distincte de la clé de consensus.

- **0016 — Posture post-quantique** 🟢
  *Direction :* documenter un chemin de migration (versioning d'adresse déjà possible via bech32, schéma de signature enfichable), sans l'implémenter maintenant. Ed25519 n'est pas PQ.

- **0017 — Arrêt d'urgence & reprise** 🟢
  *Problème :* aucun mécanisme sûr pour suspendre la chaîne sur bug critique (distinct du gel de compte, interdit par le protocole).
  *Direction :* halt coordonné par la gouvernance (pas de gel de solde), procédure de reprise/rollback documentée.

### Tokenomics & frais

- **0028 — [Partage de l'émission sur le quorum de finalité](./0028-partage-emission-quorum.md)** 🟠 **— Proposé**
  *Problème :* 100 % de l'émission va au seul proposeur ; la co-signature (qui donne la finalité) est un travail non payé, les revenus sont en grumeaux, et le *winner-take-all* + émission temporelle amplifie la concentration early (fair launch) et l'incitation à retarder.
  *Direction :* répartir l'émission de chaque bloc entre le proposeur (`PROPOSER_SHARE_BPS`, gouvernable) et les **co-signataires** qui l'ont finalisé, à parts égales (non pondéré par le bond). **Ne touche pas la courbe** (ADR 0021 immuable) : même masse par unité de temps, distribution différente. Impose de créditer à la **finalisation** (les co-sigs arrivent après la production). À `n=1` : inchangé.

- **0033 — [Genèse & bootstrap de fair-launch](./0033-genese-bootstrap-fair-launch.md)** 🟠 **— Proposé**
  *Problème :* le lancement n'est pas cadré (`GenesisConfig` = 1 validateur + 1 clé admin) ; si le set de départ est petit et gated pendant la fenêtre de forte émission, une poignée d'acteurs capte le front-loading → contredit le fair-launch (le vrai risque du Q4 : *peu* gagnent beaucoup).
  *Direction :* genèse **multi-validateurs** + comité K-of-M + `genesis_hash` empreinté ; surtout **ouverture permissionless par bond dès le départ** (le marché concourt pour les récompenses early → le set grossit quand l'émission est la plus riche). Optionnel/à débattre : plafond de forge par bloc + rampe de bond. **Ne touche pas la courbe** (0021) — agit sur *distribution/admission*. Dépend du consensus n≥3 éprouvé.

- **0004 — Invariant exécutable** 🔴 **— ✅ implémenté**
  *Fait :* `supply_invariant_holds()` (`circulation + Fonderie == MAX`) appliqué comme **garde dure** sur tous les chemins de bloc (producer → refus de sceller ; P2P/sync → rollback), car le state_root ne couvre pas la Fonderie. Ajout de `credit_from_foundry_for_test` pour les setups de test.

- **0009 — Frais des transactions stake/unstake** 🟠 **— ✅ implémenté** (option retenue : exemption assumée + **plafond de déliaisons par compte** contre le spam — plus robuste qu'un micro-frais ; whitepaper réconcilié)
  *Problème :* le whitepaper donne un poids `1` à stake/unstake, mais le code les **exempte** (fee ZERO). Incohérence + petit vecteur de spam.
  *Direction :* décider — soit facturer le forfait (aligne le whitepaper), soit assumer l'exemption et corriger le whitepaper. Traiter l'anti-spam.

- **0021 — Immutabilité de la courbe d'émission** 🟠 **— ✅ implémenté (halving) · amendé par 0040**
  *Problème :* le halving (8 ans) et le total sont des constantes ; leur statut (gouvernable ou gravé) n'est pas décidé.
  *Direction :* graver l'émission comme **immuable** (argument de confiance : personne, pas même l'admin, ne change la politique monétaire). **Amendement (0040) :** l'objet immuable devient la **loi élastique `E = r·F`** (`r = 7 %`), plus le halving discret — on grave une *loi*, pas une *courbe*. Test constitutionnel à mettre à jour dans la tranche 0040.

- **0040 — [Émission élastique à réservoir](./0040-emission-elastique-reservoir.md)** 🔴 **— Proposé** (constitutionnel, remplace le halving)
  *Problème :* la courbe à halving (1) ne distribue qu'aux **validateurs** (le public n'a aucun moyen d'obtenir des jetons sans les leur acheter) et (2) est rigide/déconnectée de l'usage.
  *Direction :* **débit `E = r · F`** — on émet une fraction `r` de la Fonderie restante ; le **melt** (VINX consommés) **recycle vers la Fonderie**. `dC/dt = r(MAX−C) − M` → **auto-régulation** vers `C* = MAX − M/r`, `E* = M` (l'émission égale l'usage). Circulation **hard-cappée ET élastique** ; **amorçage gratuit** (Fonderie pleine finance l'an 1 sans usage) ; **cas sans usage sûr** (dégénère en exponentielle). `r = 7 %/an` retenu (simulé, `scripts/emission_sim.py`). Alimente les reward pools des subnets (0039), réparti par l'usage (0041) ; les validateurs vivent des **frais**. **Amende 0021** (on grave une *loi*, plus une *courbe*).

- **0041 — [Répartition de l'émission entre subnets par l'usage (melt)](./0041-repartition-emission-usage-melt.md)** 🔴 **— Proposé**
  *Problème :* 0040 fixe **combien** on émet ; reste **vers quels subnets**, sans comité, sans tokens de subnet/AMM, sans scoring subjectif on-chain.
  *Direction :* émission d'une fenêtre répartie **au prorata du VINX melté (usage réel)** par subnet, sous **plafond `CAP`** et **porte de bond** `MIN_SUBNET_BOND`. Le melt objective l'inter-subnet ; la distribution intra-subnet reste au `reward_root` + bond (0023). **Rejette le staking à la TAO** (spéculation/plutocratie). Sybil-résistant tant que melt réel ≫ émission (garde-fous à l'amorçage). Boucle fermée non-spéculative : *dépenser pour un service dirige l'émission vers ses fournisseurs.*

- **0038 — [Bloc heartbeat périodique](./0038-bloc-heartbeat-periodique.md)** 🟠 **— ✅ implémenté**
  *Problème :* production à la demande + émission intégrée sur le temps + frais 100 % au producteur ⇒ le leader force un bloc **gratuitement** (self-tx) pour capturer l'accrual ; à l'équilibre, tous spamment → production continue de tx bidon (~2-3,5 Go/an) et prime jackpot au réveil ; MTP périmé au repos.
  *Fait :* **au moins un bloc toutes les 10 min** (`HEARTBEAT_INTERVAL_SECS = 600`), même vide (~15-30 Mo/an) ; cadence à la demande conservée sous charge. L'accrual est forgé à heure fixe par le leader round-robin → l'incitation au spam disparaît, la distribution est lissée (complété par 0028). Pas de plafond d'accrual (courbe 0021 intacte — doctrine actée dans l'ADR).

### Données, état & scaling

- **0026 — [Dépôt existentiel (anti-bloat de l'état)](./0026-depot-existentiel.md)** 🔴 **— ✅ implémenté** (consensus-breaking)
  *Problème :* `apply_transfer` matérialisait un compte (60 o **définitifs**) pour n'importe quel solde, même 1 atom → **inflation d'état à coût quasi nul**, seul terme non borné du stockage (la simulation le confirme : ~33 des 48 GB à 30 ans en régime saturé sont de l'état).
  *Fait :* solde plancher `EXISTENTIAL_DEPOSIT_ATOMS` (0,001 VinX) **gravé** — un transfert laissant une partie dans `]0, ED[` est rejeté (`BelowExistentialDeposit`), validé **avant toute mutation** (le producteur ne rollback pas une tx échouée) ; un compte vidé à `0` (sans stake ni déliaison) est **reapé** — retiré de la map *et effacé du store* (pas de résurrection au reload). Compte staké exempté du plancher, jamais reapé tant que `staked > 0`. `circulating_supply` inchangé (ADR 0004 tient). Test-tripwire constitutionnel sur la constante.

- **0035 — [Bornes de ressources par transaction](./0035-bornes-ressources-transaction.md)** 🟠 **— Proposé**
  *Problème :* la machine d'état ne borne **aucune** ressource d'une tx individuelle — un `payload` peut aller jusqu'à la borne P2P (16 Mio) à **fee forfaitaire plat** (le fee ne price pas la taille) → bloat/DoS à coût faible.
  *Direction :* plafond de taille de payload par type de tx + **poids de bloc borné** (pas seulement le nombre de tx), gravés et vérifiés avant mutation ; éventuelle composante fee par octet pour les gros payloads (sans casser le forfait plat du paiement nu). Défense en profondeur sous la borne transport (0022), poids prévisible pour la vérif parallèle (0015).

- **0013 — Cycle de vie de l'état** 🟢
  *Problème :* les comptes ne sont **jamais élagués** → croissance non bornée de l'état (la chaîne est prunée, pas l'état).
  *Direction :* expiration/rent d'état, nœuds d'archive, ou compaction des comptes dormants à solde nul. Le **dépôt existentiel (ADR 0026)** en est la première tranche ; ce ADR couvre le loyer d'état / resurrection au-delà.

- **0014 — Standard light-client** 🟠
  *Direction :* formaliser le format de preuve (état Merkle — déjà là — + chaîne d'en-têtes + preuve de finalité), et la *weak subjectivity* / checkpoints pour un fast-sync sûr.

- **0015 — [Exécution parallèle & vérification parallèle](./0015-execution-parallele.md)** 🟢 **— ✅ tranche 1 (vérif) implémentée ; exécution différée**
  *Fait (tranche 1) :* vérification **parallèle** (rayon) de toutes les signatures de tx sur les chemins de validation de bloc (P2P `NewBlock`/`SyncResponse`, sync au démarrage), puis application d'état **séquentielle** trusted. Coût CPU dominant passé de 1 à *N* cœurs, **zéro risque consensus** (vérif pure, order-independent). `WorldState::verify_tx_signature_pure`.
  *Différé (tranche 2) :* exécution d'état parallèle (Block-STM) — pertinente seulement à des dizaines de milliers de TPS soutenus, consensus-critique. Critères de déclenchement listés dans l'ADR (banc 3-validateurs + profilage + besoin réel).

- **0020 — [Sérialisation canonique consensus-critique](./0020-serialisation-canonique.md)** 🟠 **— ✅ tranches 1 & 2**
  *Fait :* **vecteurs dorés hex** (octets exacts figés) pour `GovernanceAction` (6 variants), `ModuleOp` (3 variants) et `BlockHeader::hash` (message co-signé), en plus des round-trips canoniques (`SlashEvidence`) et des vecteurs `signing_bytes` pré-existants. Attrape un décalage de discriminant / réordonnancement de champ qu'un round-trip laisse passer. Audit : bincode déterministe (LE, discriminants u32, longueurs u64), registre de modules en `BTreeMap` (ordre canonique, jamais `HashMap`).

### Réseau P2P

- **0022 — [Durcissement P2P / anti-DoS](./0022-durcissement-p2p.md)** 🟠 **— ✅ tranche 1 implémentée**
  *Fait :* garde **anti-bombe de décompression** (borne la sortie zstd à 16 Mio — fermait un OOM à un seul message), `max_transmit_size` explicite, bornes de sync (cap 512 blocs + budget 8 Mio + `saturating_add` anti-panique), et **rate-limiting par pair** (token bucket) + réputation/ban formalisés (`p2p::guard`).
  *Différé (tranche 2) :* peer-scoring de mesh gossipsub natif (décroissance temporelle), pénalité sur contenu sémantiquement invalide (verdict de dispatch), fast-sync par checkpoints (⇄ 0014), rate-limit par IP/sous-réseau (anti-Sybil transport).

- **0037 — [Propagation compacte des blocs](./0037-propagation-compacte-blocs.md)** 🟢 **— Proposé**
  *Problème :* le bloc est diffusé **plein** alors que ses tx ont déjà transité via `NewTransaction` et sont dans le mempool des pairs → **bande passante doublée** et latence accrue à l'échelle.
  *Direction :* `CompactBlock` (en-tête + signatures + hachages de tx), reconstruction locale depuis le mempool, `GetBlockTxs` pour les manquantes, repli sur `NewBlock` plein. Additif/rétro-compatible ; gain surtout avec grand N (⇄ 0029). Priorité future.

### Gouvernance

- **0007 — [Unification des chemins de gouvernance](./0007-unification-gouvernance.md)** 🟠 **— ✅ implémenté** (consensus-breaking)
  *Problème :* `AddValidator`/`RemoveValidator` existaient en **type de tx** (0x05/0x06) **et** en `GovernanceAction` (via `AdminAction`), avec des sémantiques divergentes (erreur vs ignore silencieux sur doublon).
  *Fait :* discriminants 0x05/0x06 **retirés** ; un seul chemin `AdminAction` (0x08) avec **sémantique stricte** unique (bond requis, refus du doublon, refus du dernier validateur) et **nonce consommé seulement en cas de succès**. Wallet CLI, desktop-core et console admin JS reconstruisent `bincode(GovernanceAction)` ; golden vector mis à jour.

- **0008 — chain_id par défaut sûr** 🟠 **— ✅ implémenté**
  *Problème :* tous les constructeurs et le `serde(default)` retombent sur `DEVNET` — piège anti-replay pour une tx désérialisée sans chain_id.
  *Direction :* pas de défaut silencieux (chain_id explicite requis), ou défaut le plus sûr.

- **0011 — [Décentralisation de la gouvernance](./0011-decentralisation-gouvernance.md)** 🟠 **— ✅ tranche 1 implémentée** (consensus-breaking)
  *Fait :* **multisig K-of-M** par proposition/approbation (chaque approbation = une tx mono-signée, aucun changement de format de tx). `AdminPolicy {signers, threshold}`, `GovernanceAction::SetAdminPolicy`, exécution au seuil, purge des propositions au changement de comité. `check_admin` durci (une clé isolée ne court-circuite plus le seuil une fois le comité installé). Migration de persistance in-place (v7→v8 append). Wallet CLI déjà compatible (JSON).
  *Différé (tranche 2) :* gouvernance par les validateurs (poids par bond), expiration temporelle des propositions, signature à seuil cryptographique (BLS/FROST).

### Modules (par-dessus l'ADR 0001)

- **0010 — [Primitive d'ancrage & registre de modules](./0010-primitive-ancrage-modules.md)** 🟠 **— ✅ tranche 1 implémentée** (consensus-breaking)
  *Fait :* type de tx `AnchorState` (0x09) portant un `ModuleOp` (Register/Anchor/Deregister), registre `modules: BTreeMap<Hash32, ModuleEntry {operator, bond, anchor_head, anchored_count}>`. La L1 n'exécute **jamais** la logique de module — elle n'ancre que des commitments bondés. Bond verrouillé (neutralité de circulation, ADR 0004), min-bond + `MAX_MODULES` anti-bloat, opérateur ≥ ED (jamais reapé). Migration in-place v8→v9.
  *Différé :* adjudication de fraude / slashing du bond (**→ ADR 0023**), délai de déliaison du bond, métadonnées & commande wallet dédiée.

- **0034 — [Disponibilité des données & vérification d'ancre](./0034-disponibilite-donnees-verification-ancre.md)** 🟠 **— Proposé**
  *Problème :* 0010 ancre une **racine** mais la L1 ne stocke pas les données — **où** sont-elles, et **comment** un tiers vérifie une ancre ? Sans ça, un opérateur ancre un nombre opaque et l'ADR 0023 (slashing de fraude) n'a rien sur quoi s'appuyer.
  *Direction :* standardiser un **engagement de disponibilité** (hash du blob + backend DA, on-chain borné vs externe) et un **format de preuve d'inclusion** Merkle unique (réutilise la brique du state_root, aligné 0014). La L1 stocke l'engagement, jamais les données. Tranche 1 = engagement + preuve d'inclusion ; tranche 2 = contestation d'indisponibilité (érasure-coding). **Prérequis d'ADR 0023.**

- **0023 — Adjudication du slashing de module** 🟢
  *Direction :* comment une fraude d'opérateur de module est prouvée et sanctionnée (bond → réputation → preuves de fraude → zk), sans jamais exécuter la logique du module sur la L1. **Dépend d'ADR 0034** (DA + preuve pour qu'une fraude soit prouvable).

- **0039 — [Infrastructure de subnets : escrow bondé + racine de récompense](./0039-infrastructure-subnets-escrow-recompense.md)** 🟠 **— Proposé**
  *Problème :* un module 0010 est **mono-opérateur** (il ancre un nombre) ; pour héberger un vrai subnet — plusieurs participants qui font un travail et **gagnent des VINX** — il manque le paiement **sans confiance** de participants multiples, jugé hors-chaîne.
  *Direction :* un subnet = module bondé + **escrow VINX** + **racine de récompense cumulative** + **réclamation par preuve Merkle** (`Deposit`/`SetRewardRoot`/`Claim`, appendés à `ModuleOp`). La L1 ne juge jamais le travail ; dommage max borné par l'escrow ; aucune émission détournée (rejette le modèle Bittensor). Premier subnet de démo : **balise d'aléa VRF** (honnête par construction, synergie 0029). **Prérequis :** 0034 (preuves), 0023 (fraude), banc n≥3.

### Robustesse & exploitation

- **0006 — [Préavis d'upgrade en temps réel](./0006-preavis-upgrade-temps-reel.md)** 🟠 **— ✅ implémenté** (consensus-breaking)
  *Fait :* l'activation d'upgrade passe de la **hauteur de bloc** au **timestamp** (unix secondes). Constantes `UPGRADE_NOTICE_*_SECS` (7/30/90 j), `ScheduledUpgrade.activation_ts`, `min_notice_secs()`, annonce/activation mesurées contre `current_block_ts`. Payload 14 o inchangé (octets identiques, sémantique = timestamp). Wallet CLI (`--activation-ts`), desktop-core+dto, app desktop, console admin JS et SDK alignés. Ferme le dernier écart doc↔code.

- **0018 — Observabilité & SLO** 🟢
  *Direction :* formaliser métriques (déjà Prometheus/Grafana), alertes, objectifs de service, à mesure que le réseau grandit.

- **0019 — TLS natif (rustls)** 🟢
  *Direction :* HTTPS sur le RPC sans dépendre d'un reverse-proxy.

---

### Comment décider un ADR proposé

1. Copier le gabarit de l'ADR 0001 (Contexte / Décision / Modèle / Conséquences / Alternatives).
2. Instruire le compromis honnêtement (surtout : ce que ça coûte, pas seulement ce que ça apporte).
3. Passer le statut à `Accepté` (design) puis, à l'implémentation, référencer le commit/CHANGELOG.
