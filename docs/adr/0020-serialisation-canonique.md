# ADR 0020 — Sérialisation canonique consensus-critique

- **Statut :** Accepté — ✅ tranches 1 & 2 implémentées (vecteurs dorés hex sur tous les
  encodages consensus-critiques)
- **Catégorie :** Robustesse · **Priorité :** 🟠 moyenne
- **Date :** Juillet 2026

## Contexte

Toute suite d'octets qui entre dans un **hash signé** doit avoir un **encodage canonique
unique** — sinon deux implémentations (ou deux versions) peuvent diverger, et une
malléabilité devient possible.

État actuel :
- `Transaction::signing_bytes`, `BlockHeader::hash`, `hash_account` sont **écrits à la
  main** en big-endian à largeur fixe → déterministes et verrouillés par vecteurs dorés.
  **C'est bon.**
- Mais des **payloads** consensus-pertinents passent par `bincode`/`borsh` :
  `SlashEvidence`, `GovernanceAction`, et le format wire P2P. Leur canonicité n'est pas
  formellement garantie (ordre de map, largeurs, variantes d'enum, absence de flottant).

## Décision proposée

- **Auditer et figer** l'encodage de toutes les structures consensus-critiques
  (payloads inclus). Interdire tout sérialiseur non déterministe sur le hot-path.
- Ajouter des **vecteurs dorés** pour les payloads (SlashEvidence, GovernanceAction) comme
  pour `signing_bytes`.
- **Documenter** la disposition canonique comme partie de la spec (prérequis pour une
  éventuelle 2ᵉ implémentation).

## Conséquences

- **+** Empêche la malléabilité et la divergence inter-implémentations ; condition
  nécessaire à un client alternatif ou à un audit sérieux.
- **−** Surtout du travail d'audit + tests ; coût runtime négligeable.

## État de l'implémentation

**Tranche 1** (déjà livrée) : tests de round-trip + déterminisme pour `GovernanceAction` et
`SlashEvidence`.

**Tranche 2** (cet ajout) : **vecteurs dorés hex** — les octets exacts sont figés en dur, ce
qui est strictement plus fort qu'un round-trip : cela attrape aussi un **décalage de
discriminant** ou un **réordonnancement de champ** *interne-cohérent* (qu'un round-trip laisse
passer) — précisément ce qui casserait silencieusement la vérification de signature
inter-implémentations.

Audit des chemins consensus-critiques et couverture :

| Structure | Encodage | Couverture |
|---|---|---|
| `Transaction::signing_bytes` | manuel big-endian | vecteur doré (t1 pré-existant) |
| `BlockHeader::hash` (message co-signé) | manuel big-endian | **vecteur doré hex ajouté** |
| `hash_account` (feuille du `state_root`) | manuel big-endian | déterministe par construction |
| `GovernanceAction` (payload `AdminAction`) | bincode | **vecteurs dorés hex (6 variants)** + round-trip |
| `ModuleOp` (payload `AnchorState`) | bincode | **vecteurs dorés hex (3 variants)** + round-trip |
| `SlashEvidence` (payload `SlashValidator`) | bincode | round-trip canonique (signatures non déterministes → pas de hex figé ; ses en-têtes sont re-hashés via `BlockHeader::hash`, lui figé) |
| Format wire P2P | borsh + zstd | round-trip (messages.rs) ; décodage **borné** anti-bombe (ADR 0022) |

Constats d'audit (déterminisme garanti) :
- **bincode** (config par défaut) : entiers à largeur fixe little-endian, discriminants d'enum
  en `u32`, `Vec`/map préfixés par une longueur `u64` — pas de flottant, pas d'ambiguïté.
- Le registre de modules (ADR 0010) utilise une **`BTreeMap`** (ordre de clés déterministe) —
  **jamais** une `HashMap`, dont l'ordre d'itération casserait la canonicité.
- Les nouveaux variants (`SetAdminPolicy`, `ModuleOp`, `TransactionType::AnchorState`) sont
  **appendés** → les discriminants existants sont inchangés, épinglés par les vecteurs.

## Alternatives écartées

- **Supposer que « ça marche »** : rejeté — le consensus exige des garanties de
  déterminisme, pas des suppositions.

## Notes d'implémentation

Vérifier que borsh (déjà utilisé sur le wire) est bien déterministe pour nos types ;
préférer borsh à bincode pour les payloads signés ; ajouter des tests de round-trip +
vecteurs dorés. Lister explicitement dans la spec chaque champ et sa largeur.
