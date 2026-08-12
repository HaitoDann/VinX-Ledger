# ADR 0039 — Rémunération des opérateurs de modules (escrow + partage on-chain)

- **Statut :** Accepté (décision de design) — **non implémenté** à ce jour.
- **Catégorie :** Modules & Économie · **Priorité :** 🟠 moyenne
- **Date :** Août 2026
- **Liens :** s'appuie sur le registre de modules bondés (ADR 0010) ; nécessite la DA &
  preuve d'ancre (ADR 0034) pour la vérification on-chain de la livraison ; cohérent avec
  l'émission progressive (ADR 0040) — **aucune émission supplémentaire** pour les modules.

---

## 1. Contexte

ADR 0010 a posé la primitive d'ancrage : un opérateur bonde des VINX, enregistre un module,
et ancre des racines Merkle via `AnchorState`. Ce qui manque : **comment est-il rémunéré ?**

Deux écueils à éviter :
1. **Émission secondaire** (type Commerce Pool) : gameable par du volume artificiel, sort
   l'émission du périmètre sécurité du consensus → **rejeté**.
2. **Pas de rémunération** : un module sans revenu n'a aucune raison économique d'exister
   → les opérateurs ne bondent pas.

La seule source de revenu légitime : **les utilisateurs du service** paient pour ce qu'ils
consomment. Le L1 route le paiement de façon déterministe, sans exécuter la logique du
module.

---

## 2. Décision

**Les services des modules sont payés par leurs clients via un escrow on-chain**, libéré
automatiquement à la preuve de livraison. Le L1 répartit le paiement selon un tableau de
bénéficiaires enregistré à la genèse du module — **marché libre** : chaque module fixe son
propre prix et sa propre structure de partage.

### 2.1 Structure d'un module (ajout à l'enregistrement ADR 0010)

À la registration (`AnchorState` avec `ModuleOp::Register`), le module définit :

```
fee_schedule: {
    base_fee_atoms: u128,          // tarif de base d'une unité de service (indicatif)
    recipients: Vec<{              // bénéficiaires du paiement à la livraison
        address: Address,
        share_bps: u16,            // fraction en points de base (10 000 = 100 %)
    }>,
    // sum(share_bps) ≤ 10 000 ; le résidu va à l'adresse de l'opérateur (bond owner)
    operator_address: Address,     // reçoit le résidu + fait les interactions on-chain
}
```

Le tableau `recipients` modélise naturellement la structure interne du module. Exemple :
client paie 100 VINX pour 100 Go de stockage auprès d'un module avec 3 providers :

```
recipients: [
    { address: provider_A, share_bps: 3000 },   // 30 VINX
    { address: provider_B, share_bps: 3000 },   // 30 VINX
    { address: provider_C, share_bps: 3000 },   // 30 VINX
]
operator_address: coordinateur                   // résidu = 1000 bps → 10 VINX
```

Le L1 n'a pas besoin de connaître la nature du service ou la relation entre l'opérateur et
ses providers — il route le paiement selon le registre, point.

### 2.2 Cycle de vie d'un paiement (escrow)

#### Étape 1 — Ouverture de l'escrow (client)

Le client soumet une transaction `ModuleEscrow` (nouveau type `0x0A`) :

```
ModuleEscrow {
    module_id: Hash32,             // identifiant du module (hash de l'entrée registre)
    service_params_hash: Hash32,   // engagement sur les paramètres du service (off-chain)
    amount_atoms: u128,            // montant bloqué
    delivery_timeout_secs: u64,    // délai de livraison accepté (entre MIN et MAX borné)
}
```

- Le montant est débité du compte client **immédiatement** et stocké dans
  `pending_escrows[escrow_id]` (`WorldState`).
- L'`escrow_id` = `hash(client_address || module_id || nonce_client)` — déterministe,
  sans collision.
- Le module est notifié hors-chaîne (via l'index de son ancre ou un webhook opérateur).

#### Étape 2 — Livraison et preuve (module)

L'opérateur livre le service off-chain. Il ancre ensuite une racine Merkle via `AnchorState`
**incluant une feuille `EscrowRelease { escrow_id }`**. La présence de cette feuille dans
la racine ancrée constitue la **preuve de livraison on-chain**.

> ADR 0034 (DA & preuve d'ancre) définit la structure exacte de la preuve vérifiable.
> Jusqu'à l'implémentation de 0034, la preuve peut être auto-déclarée par l'opérateur
> (niveau de confiance initial : bond + réputation) et ajustée à la hausse plus tard.

#### Étape 3 — Libération (déclenchée par la preuve)

Quand le nœud détecte la feuille `EscrowRelease { escrow_id }` dans un `AnchorState`
finalisé, il exécute la distribution atomique :

```
pour chaque bénéficiaire r dans fee_schedule.recipients :
    paiement_r = floor(escrow.amount_atoms × r.share_bps / 10_000)
    crediter(r.address, paiement_r)

résidu = escrow.amount_atoms - Σ paiements_r
crediter(fee_schedule.operator_address, résidu)

supprimer pending_escrows[escrow_id]
```

La troncature entière va à l'opérateur (résidu déterministe, zéro perte d'atomes).

#### Étape 4a — Remboursement (timeout, option client)

Si `block_timestamp ≥ escrow.created_ts + delivery_timeout_secs`, le client peut soumettre
`ModuleEscrowRefund { escrow_id }` pour récupérer son dépôt.

**Évaluation paresseuse** : le L1 ne balaie pas les escrows à chaque bloc (O(n) prohibitif) ;
le client déclenche lui-même le remboursement. Les escrows expirés non réclamés restent dans
l'état jusqu'au `ModuleEscrowRefund` — ils n'ont aucun effet économique en attendant.

#### Étape 4b — Pénalité (bond) en cas de défaut répété

Un timeout simple (client non livré) ne déclenche **pas** de slash automatique — le réseau
ne peut pas distinguer un client fantôme d'un opérateur défaillant. En revanche, si un
mécanisme de **rapport de défaut** (futur ADR, lié à ADR 0023 slashing de fraude) établit
une preuve de défaillance de l'opérateur, le bond peut être slashé.

### 2.3 Nouveaux types de transaction

| Code | Type | Rôle |
|------|------|------|
| `0x0A` | `ModuleEscrow` | Crée un escrow (client → module) |
| `0x0B` | `ModuleEscrowRefund` | Rembourse un escrow expiré (client) |

La libération (étape 3) n'est **pas** une transaction séparée — c'est un effet de bord
déterministe de l'application d'un `AnchorState` contenant la feuille `EscrowRelease`.

### 2.4 Bornes et paramètres

| Paramètre | Gouvernable ? | Valeur indicative | Rôle |
|---|---|---|---|
| `MODULE_ESCROW_MIN_TIMEOUT_SECS` | Oui | 3 600 (1 h) | Délai minimum — protège l'opérateur |
| `MODULE_ESCROW_MAX_TIMEOUT_SECS` | Oui | 2 592 000 (30 j) | Délai maximum — borne l'état |
| `MODULE_ESCROW_MAX_OPEN` | Oui | 10 par client par module | Anti-spam état |

---

## 3. Conséquences

**Positif**
- **Aucune émission secondaire** : la rémunération vient entièrement de la valeur créée
  pour les utilisateurs — pas de jeu possible sur le protocole.
- **Marché libre** : chaque module fixe son `fee_schedule` ; la concurrence entre modules
  régule les prix naturellement.
- **Structure interne opaque pour le L1** : les 3 providers, le coordinateur, les commissions
  — tout ça est dans le `recipients`, le L1 ne distingue pas les rôles.
- **Garantie client** : le paiement est en escrow — si le module ne livre pas, le client
  récupère ses VINX.
- **Incitation à l'exactitude** : l'opérateur a intérêt à ancrer la preuve rapidement ;
  son bond est sa caution en cas de comportement systématiquement défaillant.
- **Compatible avec ADR 0028 (époque)** : les paiements d'escrow sont des transfers
  ordinaires, pas de l'émission — ils ne passent pas par le mécanisme d'époque.

**Coûts / pièges**
- **État `pending_escrows`** : champ supplémentaire dans `WorldState`, croît avec le nombre
  d'escrows ouverts → bornage nécessaire (`MODULE_ESCROW_MAX_OPEN`) + bump `STORAGE_VERSION`.
- **Dépendance à ADR 0034** : sans preuve vérifiable on-chain, la libération repose sur
  la confiance dans l'opérateur (bond comme caution). Acceptable en phase 1 ; à durcir.
- **Pas de refund automatique** : les clients doivent surveiller leurs escrows et soumettre
  `ModuleEscrowRefund`. Interface à documenter côté wallet et SDK.
- **Fee schedule immuable** : une fois enregistré, le `recipients` et `operator_address` ne
  changent qu'à la re-registration du module (nouveau bond, nouveau module_id). Implique une
  migration d'escrows ouverts → à définir dans la procédure de re-registration.

---

## 4. Alternatives écartées

- **Émission dédiée aux modules** (type Commerce Pool) : rejeté — gameable (volume
  artificiel), ne crée pas de valeur réelle, dilue la supply sans contrepartie.
- **Paiement direct (sans escrow)** : rejeté — le client paie sans garantie ; sans smart
  contract pour conditionner la livraison, il n'y a aucun recours. Le bond seul est
  insuffisant (il couvre une fraude prouvée, pas une non-livraison banale).
- **Paiement post-livraison** : rejeté — le module livre sans garantie de paiement ; sans
  mécanisme d'enforcement, les clients peuvent ne pas payer.
- **Partage uniquement off-chain** (opérateur redistribue manuellement) : rejeté — opaque,
  non vérifiable, crée une relation de confiance entre providers et opérateur que le L1 ne
  peut pas auditer.
- **Pondération des parts par le bond** : rejeté — favorise les opérateurs capitalisés,
  casse le marché libre.

---

## 5. Notes d'implémentation

- `vinx-core` :
  - Ajout tx `ModuleEscrow` (`0x0A`) et `ModuleEscrowRefund` (`0x0B`).
  - `EscrowEntry { escrow_id, client, module_id, amount_atoms, created_ts, timeout_secs, service_params_hash }`.
  - Mise à jour `ModuleEntry` (ADR 0010) avec `fee_schedule: FeeSchedule`.
- `vinx-state` :
  - `pending_escrows: BTreeMap<Hash32, EscrowEntry>` dans `WorldState`.
  - `apply_module_escrow` : débit client, création `EscrowEntry`.
  - `apply_anchor_state` : si racine contient `EscrowRelease { escrow_id }`, exécuter la
    distribution atomique puis supprimer l'entrée.
  - `apply_escrow_refund` : vérifier timeout, créditer client, supprimer l'entrée.
- Bump `STORAGE_VERSION` : `pending_escrows` appendé.
- Tests : distribution exacte (résidu → opérateur, zéro perte), remboursement après timeout,
  refus avant timeout, preuve invalide ignorée, max escrows par client.
- Dépendances : ADR 0010 (registre), ADR 0034 (preuve vérifiable, phase 2).
