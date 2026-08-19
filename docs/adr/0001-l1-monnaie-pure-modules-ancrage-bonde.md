# ADR 0001 — L1 monnaie pure + surcouches par ancrage bondé

- **Statut :** Accepté — **partiellement implémenté** (registre bondé + ancrage Merkle) ; vérification ZK SP1 = ADR 0050 (architecture cible).
- **Date :** Juillet 2026 · Révisé août 2026
- **Portée :** Architecture d'extensibilité de VinX Ledger — doctrine fondatrice.
- **Décideur :** VinX Labs.

---

## 1. Contexte

VinX doit pouvoir accueillir des fonctionnalités variées (émission de sous-actifs,
traçabilité, launchpads, canaux de paiement…) **sans** compromettre ce qui fait sa
valeur : une **monnaie pure**, souveraine, minimale, sans smart contracts dans le
cœur, ultra-auditable en Rust.

La question : comment ajouter de la variété applicative **sans salir la base** ?

## 2. Décision

On adopte le modèle **« monnaie pure (L1) + surcouches par ancrage bondé »**.

> **La L1 n'exécute jamais de logique applicative.** Elle reste une monnaie pure,
> augmentée d'exactement deux capacités : un **journal d'ancrages** (enregistrer un
> engagement cryptographique) et un **registre d'opérateurs bondés**. Toute la
> complexité vit dans des **modules hors-nœud**.

Concrètement, la L1 ne connaît des modules que trois choses :

1. **Qui** les opère (un opérateur, identifié par une adresse).
2. **Combien** de VINX il a mis en gage (**bond**, réutilisant le mécanisme de
   bond/slashing existant).
3. **Le dernier hash** d'état qu'il a ancré (`anchor_head`).

Elle n'exécute, ne valide et ne comprend **rien** de la logique interne d'un module.

### Pourquoi l'ancrage (et pas les autres options)

- **Rejeté — modules dans le nœud (micro-kernel / plugins validateurs).** Un module
  exécuté *dans* le process du nœud peut le faire paniquer, casser le déterminisme du
  consensus ou fuir de la mémoire. Cela **contredit** l'objectif « ne pas salir le
  cœur » à moins d'un bac à sable WASM métré — soit exactement la complexité de VM que
  VinX refuse. **Interdit par cette ADR.**
- **Retenu (V1+) — appchains / subnets.** Les Appchains sont des modules hors-L1 qui génèrent
  des preuves SP1 et les soumettent à la L1 pour vérification + settlement. La messagerie
  inter-chaînes passe par le Clearinghouse L1 (ADR 0049). C'est la direction choisie —
  la primitive d'ancrage bondé en est le socle.
- **Retenu — ancrage d'état + règlement (rollup/sidechain minimaliste).** Le module
  tourne dans un **autre programme** : un bug chez lui ne peut littéralement pas toucher
  le nœud. Surface L1 minuscule. C'est le socle ; les appchains pourront s'y greffer.

## 3. Le contrat L1 (surface ajoutée — volontairement minuscule)

```
   [ Module Token Factory ]   [ Module Traçabilité ]   [ Module … ]
        (hors-nœud)                (hors-nœud)
            │  ancre un hash            │
            │  règle des VINX           │
            ▼                           ▼
   ┌───────────────────────────────────────────────────────┐
   │  COUCHE 1 — VinX monnaie pure (Rust / PoA Threshold)   │
   │  • Comptes VINX, consensus, émission, bond/slashing    │
   │  • + Registre de modules { operator, bond, anchor_head}│
   │  • + 1 type de tx : AnchorState (écrit un hash)        │
   │  • 0 exécution de logique de module                    │
   └───────────────────────────────────────────────────────┘
```

Trois ajouts, et pas un de plus :

1. **Registre de modules** (dans le `WorldState`, géré par gouvernance) :
   `ModuleEntry { operator: Address, bond: Amount, anchor_head: Hash32, anchored_at: u64 }`.
   Enregistrement/retrait via `GovernanceAction::RegisterModule / DeregisterModule`.

2. **Un seul nouveau type de transaction — `AnchorState` (0x09).**
   - Payload : `module_id_hash(32) ‖ commitment(32)`.
   - Règles de validation : l'émetteur `from` est l'opérateur enregistré du module ;
     le bond est ≥ au minimum requis. C'est tout.
   - Effet : `anchor_head ← commitment`, `anchored_at ← height`. **Une écriture de
     hash. Aucune exécution.**

3. **Dépôts / retraits = transferts VINX normaux.** Pas de primitive nouvelle : les
   VINX entrent et sortent d'un module via des `Transfer` vers/depuis le compte L1 du
   module. L'émission, les frais, le consensus : **inchangés**.

Le **bond** réutilise tel quel le mécanisme construit pour les validateurs (mise en
gage + slashing sur preuve). Un opérateur malhonnête est slashable par gouvernance.

## 4. Modèle de confiance (ce que la L1 garantit — et ce qu'elle ne garantit pas)

| La L1 **garantit** | La L1 **ne garantit pas** |
|---|---|
| L'`anchor_head` a bien été posé par l'opérateur bondé (signature). | Que l'état ancré a été calculé honnêtement. |
| Toute sortie de VINX est un transfert valide (solde, nonce, signature). | La correction de la logique interne du module. |
| L'historique des ancres est immuable et ordonné. | La disponibilité des données du module. |

**Conséquence directe :** « la sécurité de VinX » protège le **VINX réglé**, pas la
logique applicative. Les utilisateurs d'un module font confiance à **l'opérateur du
module** (adossé à son bond), pas magiquement à VinX.

**Sortie de fonds (le vrai point dur), par ordre de robustesse croissante :**
1. **Bond + réputation** (mode « Bondé » — disponible) : l'opérateur bonde ; une fraude prouvée
   le slashe. Simple, mais pas trustless. Pour les modules légers / expérimentation.
2. **Preuves de fraude** (rollup optimiste, futur) : n'importe qui peut prouver une
   transition invalide contre une ancre → slash automatique.
3. **Preuves de validité SP1** (zkVM RISC-V — **architecture cible V1, ADR 0050**) : l'ancre
   est accompagnée d'une preuve Groth16 que l'état est cryptographiquement correct. Le L1
   vérifie la preuve sans exécuter la logique. C'est ce qui fait de VinX un **settlement
   layer ZK-natif**.

Cette ADR acte le **niveau 1** comme point de départ accessible, et le **niveau 3** comme
architecture cible sans changer la primitive L1 (« un hash + un bond + une preuve optionnelle »).
Les deux modes coexistent — voir ADR 0050 pour les détails d'implémentation.

## 5. Exemple travaillé — le module « Token Factory »

Objectif : permettre à quiconque d'émettre des **sous-actifs** (« jetons ») sur un
registre géré par le module, **sans aucune VM dans la L1**.

### Acteurs
- **Opérateur** : fait tourner le service token-factory (hors-nœud). Enregistré dans le
  registre L1, bondé (ex. 100 000 VINX).
- **Émetteur (Alice)** : veut créer le jeton « ACME ».
- **Utilisateurs** : détiennent et s'échangent de l'ACME *dans le module*.

### Cycle de vie

```
(1) Enregistrement (une fois)
    Gouvernance: RegisterModule{ id="token-factory", operator, min_bond }
    Opérateur: bonde des VINX (réutilise le bond)              ── touche la L1

(2) Création d'un jeton
    Alice paie un petit frais VINX au compte L1 du module (Transfer)
    Alice dit au module (API hors-nœud): "créer ACME, supply 1M, admin=Alice"
    Le module inscrit ACME dans SON registre                  ── ne touche PAS la L1

(3) Vie du jeton
    Bob et Carol s'échangent de l'ACME via l'API du module
    → des milliers de tx internes, plein débit du module      ── ne touche PAS la L1
    → zéro pollution du mempool VinX

(4) Ancrage (périodique, ex. toutes les N secondes)
    Le module calcule la racine de Merkle de TOUT son état
    (soldes de tous les sous-jetons)
    Opérateur: AnchorState{ id, commitment = racine }         ── touche la L1
    L1: anchor_head ← racine

(5) Vérification (sans confiance dans le module)
    N'importe qui, avec les données du module, prouve son solde ACME
    par une preuve de Merkle contre l'anchor_head lue sur la L1.
    Le module ne peut pas réécrire l'histoire sans que l'ancre diverge.

(6) Sortie / règlement (si ACME est adossé à des VINX déposés)
    L'utilisateur prouve son solde contre la dernière ancre ;
    l'opérateur libère les VINX par un Transfer L1.           ── touche la L1
    Si ACME est un jeton autonome (non adossé VINX), il n'y a rien à
    « sortir » en VINX : la L1 fournit seulement l'ancre infalsifiable
    + le bond de l'opérateur comme responsabilité.
```

### Ce que la L1 voit de tout ça
Uniquement : l'enregistrement du module, le bond, une suite de `Transfer` (frais,
dépôts, retraits), et une suite d'`AnchorState` (32 octets de hash à chaque fois). **La
L1 ne sait même pas ce qu'est « ACME ».**

### Réutilisation directe de l'existant
- Le module produit ses preuves avec **le même format Merkle** que `vinx-crypto`
  (`merkle_proof_for` / `verify_merkle_proof`) → le vérifieur light-client déjà présent
  (navigateur, SDK) fonctionne tel quel sur les états de module.
- Le **bond/slashing** des validateurs sert de responsabilité opérateur.
- Le **payload générique** des transactions accueille `AnchorState` sans toucher au
  reste.

## 6. Conséquences

**Positives**
- Monnaie pure inviolable : un exploit dans un module ne met en danger ni les VINX ni la
  émission sur la L1.
- Surface L1 minimale (1 type de tx + 1 registre) ; cœur intact.
- Débit L1 préservé : aucun calcul applicatif ne ralentit les transferts.
- Réduction (pas suppression) de la surface réglementaire : la L1 reste un protocole de
  transfert + ancrage neutre ; la logique à risque est portée par les éditeurs de
  modules. *(Réduction de surface, pas bouclier juridique — à ne pas sur-vendre.)*
- A/C (appchains, exécution avancée) restent constructibles **par-dessus** sans nouvelle
  primitive L1.

**Négatives / compromis assumés**
- Pas de composabilité atomique entre modules (prix de l'isolation).
- La L1 ne sécurise que le VINX réglé, pas la logique des modules.
- La sortie trustless (niveaux 2-3) est un chantier lourd, non ouvert maintenant.
- Disponibilité des données à la charge du module (sinon les preuves sont invérifiables).

## 7. Règle d'or (invariant de cette ADR)

> **Aucune logique applicative ne s'exécute jamais dans le nœud VinX.** Toute proposition
> qui exigerait d'exécuter du code de module dans le process du nœud (VM embarquée,
> plugins in-process, hooks de consensus applicatifs) est refusée par principe. Le seul
> pont autorisé est : *un hash ancré + un bond + des transferts VINX.*

## 8. État d'implémentation

Rien n'est codé. Socle **déjà présent** qui rend l'implémentation future petite :
bond + slashing (implémentés), arbre de Merkle + preuves (`vinx-crypto`), payload de
transaction générique, gouvernance admin (registre). Le premier jalon concret, quand il
viendra, est l'ajout du type `AnchorState` + du registre de modules + des actions de
gouvernance `RegisterModule` / `DeregisterModule`.
