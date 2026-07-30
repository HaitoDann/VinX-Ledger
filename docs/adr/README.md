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
  *Fait :* pointeur `finalized_height` explicite et **prefix-closed** (avance sur le préfixe contigu de blocs quorum-signés), mis à jour à la production et à chaque co-signature, exposé sur `/health`, avec `is_final(height)`. À n=1 la finalité est immédiate ; à n≥2 elle suit les co-signatures.
  *Reste (nécessite le banc 3-validateurs) :* refuser de bâtir au-delà d'une profondeur non finalisée, view-change formel, latence de finalité = 1 aller-retour.

- **0005 — Temps réseau robuste** 🟠 **— ✅ implémenté** (bornes de timestamp à la réception + helper Median Time Past ; câblage émission↔MTP à finir au banc n≥2)
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

- **0004 — Invariant exécutable** 🔴 **— ✅ implémenté**
  *Fait :* `supply_invariant_holds()` (`circulation + Fonderie == MAX`) appliqué comme **garde dure** sur tous les chemins de bloc (producer → refus de sceller ; P2P/sync → rollback), car le state_root ne couvre pas la Fonderie. Ajout de `credit_from_foundry_for_test` pour les setups de test.

- **0009 — Frais des transactions stake/unstake** 🟠 **— ✅ implémenté** (option retenue : exemption assumée + **plafond de déliaisons par compte** contre le spam — plus robuste qu'un micro-frais ; whitepaper réconcilié)
  *Problème :* le whitepaper donne un poids `1` à stake/unstake, mais le code les **exempte** (fee ZERO). Incohérence + petit vecteur de spam.
  *Direction :* décider — soit facturer le forfait (aligne le whitepaper), soit assumer l'exemption et corriger le whitepaper. Traiter l'anti-spam.

- **0021 — Immutabilité de la courbe d'émission** 🟠 **— ✅ implémenté** (règle immuable §9 + test constitutionnel épinglant halving/total/cap)
  *Problème :* le halving (8 ans) et le total sont des constantes ; leur statut (gouvernable ou gravé) n'est pas décidé.
  *Direction :* graver l'émission comme **immuable** (argument de confiance : personne, pas même l'admin, ne change la politique monétaire). À acter explicitement.

### Données, état & scaling

- **0026 — [Dépôt existentiel (anti-bloat de l'état)](./0026-depot-existentiel.md)** 🔴 **— ✅ implémenté** (consensus-breaking)
  *Problème :* `apply_transfer` matérialisait un compte (60 o **définitifs**) pour n'importe quel solde, même 1 atom → **inflation d'état à coût quasi nul**, seul terme non borné du stockage (la simulation le confirme : ~33 des 48 GB à 30 ans en régime saturé sont de l'état).
  *Fait :* solde plancher `EXISTENTIAL_DEPOSIT_ATOMS` (0,001 VinX) **gravé** — un transfert laissant une partie dans `]0, ED[` est rejeté (`BelowExistentialDeposit`), validé **avant toute mutation** (le producteur ne rollback pas une tx échouée) ; un compte vidé à `0` (sans stake ni déliaison) est **reapé** — retiré de la map *et effacé du store* (pas de résurrection au reload). Compte staké exempté du plancher, jamais reapé tant que `staked > 0`. `circulating_supply` inchangé (ADR 0004 tient). Test-tripwire constitutionnel sur la constante.

- **0013 — Cycle de vie de l'état** 🟢
  *Problème :* les comptes ne sont **jamais élagués** → croissance non bornée de l'état (la chaîne est prunée, pas l'état).
  *Direction :* expiration/rent d'état, nœuds d'archive, ou compaction des comptes dormants à solde nul. Le **dépôt existentiel (ADR 0026)** en est la première tranche ; ce ADR couvre le loyer d'état / resurrection au-delà.

- **0014 — Standard light-client** 🟠
  *Direction :* formaliser le format de preuve (état Merkle — déjà là — + chaîne d'en-têtes + preuve de finalité), et la *weak subjectivity* / checkpoints pour un fast-sync sûr.

- **0015 — [Exécution parallèle & vérification parallèle](./0015-execution-parallele.md)** 🟢 **— ✅ tranche 1 (vérif) implémentée ; exécution différée**
  *Fait (tranche 1) :* vérification **parallèle** (rayon) de toutes les signatures de tx sur les chemins de validation de bloc (P2P `NewBlock`/`SyncResponse`, sync au démarrage), puis application d'état **séquentielle** trusted. Coût CPU dominant passé de 1 à *N* cœurs, **zéro risque consensus** (vérif pure, order-independent). `WorldState::verify_tx_signature_pure`.
  *Différé (tranche 2) :* exécution d'état parallèle (Block-STM) — pertinente seulement à des dizaines de milliers de TPS soutenus, consensus-critique. Critères de déclenchement listés dans l'ADR (banc 3-validateurs + profilage + besoin réel).

- **0020 — Sérialisation canonique consensus-critique** 🟠 **— ✅ tranche 1** (tests-tripwire de déterminisme + round-trip canonique pour `GovernanceAction` et `SlashEvidence` ; audit complet + vecteurs dorés hex = suite)
  *Direction :* garantir l'encodage canonique (borsh/bincode) des structures qui entrent dans un hash signé, pour empêcher toute malléabilité inter-implémentations.

### Réseau P2P

- **0022 — [Durcissement P2P / anti-DoS](./0022-durcissement-p2p.md)** 🟠 **— ✅ tranche 1 implémentée**
  *Fait :* garde **anti-bombe de décompression** (borne la sortie zstd à 16 Mio — fermait un OOM à un seul message), `max_transmit_size` explicite, bornes de sync (cap 512 blocs + budget 8 Mio + `saturating_add` anti-panique), et **rate-limiting par pair** (token bucket) + réputation/ban formalisés (`p2p::guard`).
  *Différé (tranche 2) :* peer-scoring de mesh gossipsub natif (décroissance temporelle), pénalité sur contenu sémantiquement invalide (verdict de dispatch), fast-sync par checkpoints (⇄ 0014), rate-limit par IP/sous-réseau (anti-Sybil transport).

### Gouvernance

- **0007 — [Unification des chemins de gouvernance](./0007-unification-gouvernance.md)** 🟠 **— ✅ implémenté** (consensus-breaking)
  *Problème :* `AddValidator`/`RemoveValidator` existaient en **type de tx** (0x05/0x06) **et** en `GovernanceAction` (via `AdminAction`), avec des sémantiques divergentes (erreur vs ignore silencieux sur doublon).
  *Fait :* discriminants 0x05/0x06 **retirés** ; un seul chemin `AdminAction` (0x08) avec **sémantique stricte** unique (bond requis, refus du doublon, refus du dernier validateur) et **nonce consommé seulement en cas de succès**. Wallet CLI, desktop-core et console admin JS reconstruisent `bincode(GovernanceAction)` ; golden vector mis à jour.

- **0008 — chain_id par défaut sûr** 🟠 **— ✅ implémenté**
  *Problème :* tous les constructeurs et le `serde(default)` retombent sur `DEVNET` — piège anti-replay pour une tx désérialisée sans chain_id.
  *Direction :* pas de défaut silencieux (chain_id explicite requis), ou défaut le plus sûr.

- **0011 — Décentralisation de la gouvernance** 🟠
  *Direction :* clé admin unique → **signature à seuil / multisig** → éventuellement gouvernance par les validateurs. Roadmap graduelle, sans casser le modèle actuel.

### Modules (par-dessus l'ADR 0001)

- **0010 — Primitive d'ancrage & registre de modules** 🟠
  *Direction :* première tranche concrète de l'ADR 0001 — type de tx `AnchorState` (0x09), registre `ModuleEntry { operator, bond, anchor_head }`, actions `RegisterModule` / `DeregisterModule`. Réutilise bond + Merkle + payload générique.

- **0023 — Adjudication du slashing de module** 🟢
  *Direction :* comment une fraude d'opérateur de module est prouvée et sanctionnée (bond → réputation → preuves de fraude → zk), sans jamais exécuter la logique du module sur la L1.

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
