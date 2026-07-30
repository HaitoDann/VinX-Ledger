# ADR 0011 — Décentralisation de la gouvernance (multisig K-of-M)

- **Statut :** Accepté — ✅ tranche 1 implémentée (multisig à seuil) ; gouvernance par les
  validateurs différée (tranche 2)
- **Catégorie :** Gouvernance · **Priorité :** 🟠 moyenne
- **Date :** Juillet 2026
- **Liens :** s'appuie sur l'unification de la gouvernance (ADR 0007).

## Contexte

Toute la gouvernance (ajout/retrait de validateur, frais, upgrades, rotation admin) passe par
une **clé admin unique** : `admin_address: Option<Address>`, et `check_admin` exige
`tx.from == admin`. C'est un **point de défaillance et de confiance unique** — la clé peut
être perdue, volée, ou l'opérateur peut agir seul. La roadmap veut : clé unique →
**signature à seuil / multisig** → éventuellement gouvernance par les validateurs.

## Décision — tranche 1 : multisig K-of-M par proposition/approbation

Introduire un **comité admin K-parmi-M**. Chaque approbation reste une **transaction à
signature unique** (compatible avec le modèle `Transaction` existant, aucun changement de
format de tx) : une action de gouvernance s'exécute une fois qu'elle a réuni `threshold`
approbations de signataires distincts.

### Modèle d'état (ADR 0011)

```
AdminPolicy { signers: Vec<Address>, threshold: u16 }   // le comité, ou None (mono-admin legacy)
GovernanceProposal { action_hash, action, approvals }   // une action en cours d'approbation
```

Deux nouveaux champs dans `WorldState` (`admin_policy: Option<AdminPolicy>`,
`pending_governance: Vec<GovernanceProposal>`), en `serde(default)`, **ajoutés en dernier**
(voir §Persistance). Nouvelle `GovernanceAction::SetAdminPolicy { signers, threshold }` pour
installer/remplacer le comité.

### Autorisation & exécution

L'**autorité effective** est : le comité si `admin_policy` est présent ; sinon la clé
`admin_address` (mono-admin, `threshold = 1`) ; sinon dev mode (ouvert). Dans
`apply_admin_action` :

1. Le `tx.from` doit être un signataire autorisé (sinon `Unauthorized`).
2. Nonce vérifié mais **non consommé** avant succès (ADR 0007).
3. Si `threshold ≤ 1` → **exécution immédiate**. Sinon → **accumulation d'approbations** :
   l'action est identifiée par `sha256(bincode(action))` ; les approbations distinctes sont
   comptées ; la N-ième qui atteint le seuil **exécute** l'action et purge la proposition.

Chaque arm valide **avant** de muter, et les mutations n'ont lieu qu'après validation
complète — donc une action rejetée ne consomme pas de nonce (le producteur saute une tx
échouée **sans rollback**, cf. ADR 0026). La ré-approbation par le même signataire est
rejetée. `SetAdminPolicy` purge les propositions en cours (l'ensemble éligible change).

**Fermeture de contournement.** Dès qu'un comité est installé, `check_admin` **désactive**
les raccourcis mono-admin (p.ex. la tx dédiée `AnnounceUpgrade`) : une clé isolée ne doit pas
court-circuiter le seuil — tout passe par le chemin K-of-M.

### Bornes anti-bloat

`MAX_ADMIN_SIGNERS = 64`, `MAX_PENDING_GOVERNANCE = 64` : bornent l'état qu'un comité peut
accumuler (signataires, propositions partiellement approuvées).

### Persistance (bincode ↔ migration)

`serialize_meta` fait `bincode(&WorldState)` (non auto-descriptif). Les deux nouveaux champs
sont les **derniers champs sérialisés**, donc un blob pré-0011 (v7) est un **préfixe strict**
d'un blob v8. La migration `STORAGE_VERSION 7 → 8` se contente d'**ajouter** l'encodage par
défaut (`admin_policy = None`, `pending_governance = []`) au blob — aucun wipe, comptes et
chaîne intacts. Le snapshot JSON (`serde`, auto-descriptif) porte les champs nativement ;
un ancien snapshot importe avec les défauts.

### Clients

Le wallet CLI `admin-action --action '<JSON>'` désérialise un `GovernanceAction` générique →
`SetAdminPolicy` est **immédiatement utilisable** :
`--action '{"SetAdminPolicy":{"signers":["vinx1…","vinx1…"],"threshold":2}}'`. Aucun
changement client requis.

## Conséquences

**Positif**
- Supprime le point de défaillance unique : K signatures distinctes requises.
- Migration graduelle sans rupture : un déploiement mono-admin installe un comité par une
  simple `SetAdminPolicy`, puis toute action exige le seuil.
- Fenêtre de sécurité : le comité peut se remplacer lui-même (rotation de clés compromises).

**Coûts / limites**
- Consensus-critique (l'autorisation entre dans la transition d'état) et changement de
  format d'état → **bump de version + migration** (fait, in-place).
- Les propositions vivent dans le meta (comme `pending_unbonds`), hors `state_root` — cohérent
  avec l'existant, dérivé déterministiquement de l'historique des tx.
- Pas d'**expiration temporelle** des propositions en tranche 1 (bornées par un cap). Une
  proposition jamais complétée reste jusqu'à un `SetAdminPolicy` ou l'atteinte du cap.

## Tranche 2 (différée)

- **Gouvernance par les validateurs** : dériver l'autorité du set de validateurs (poids par
  bond), plutôt qu'un comité nommé — étape finale de la roadmap.
- **Expiration des propositions** (par timestamp, façon ADR 0006) pour purger les votes
  abandonnés sans attendre le cap.
- **Signature à seuil cryptographique** (BLS/FROST) : une seule signature agrégée au lieu de
  K transactions — réduit la taille on-chain, mais lourd ; le modèle par approbations
  est le minimum viable décentralisé.

## Alternatives écartées

- **Multi-signatures dans une seule `Transaction`** : imposerait de porter K signatures dans
  la struct tx (invasif, casse le format). Le modèle par approbations réutilise la tx
  mono-signée existante.
- **Ne rien faire (mono-admin)** : rejeté — point de confiance/défaillance unique, contraire
  à l'objectif de décentralisation.

## Notes d'implémentation

- `crates/vinx-core/src/governance.rs` : `GovernanceAction::SetAdminPolicy` (appendé →
  discriminant 5, encodages existants inchangés) ; test canonique étendu.
- `crates/vinx-state/src/world_state.rs` : `AdminPolicy`, `GovernanceProposal`, champs
  `admin_policy`/`pending_governance`, `effective_admin`, `record_governance_approval`,
  `execute_governance_action`, `validate_admin_policy`, durcissement de `check_admin`,
  `v8_meta_suffix`. Tests : seuil, non-autorisé, double approbation, installation depuis
  mono-admin, validation de policy, purge des propositions au changement de comité.
- `crates/vinx-node/src/storage.rs` : `STORAGE_VERSION 8` + migration append v7→v8 ; tests
  de migration mis à jour (préfixe v7 reconstitué).
