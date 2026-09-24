# ADR 0081 — Décisions irréversibles avant la genèse

- **Statut :** Accepté ✅ — implémenté
- **Date :** Septembre 2026
- **Portée :** Protocole — tout ce qui ne peut plus changer une fois le mainnet lancé
- **Décideur :** VinX Labs (mainteneur)
- **Remplace / modifie :** ADR 0021 (valeur de la supply), ADR 0040 (plafond), ADR 0038 (bond),
  ADR 0010 (registre de modules, retiré), ADR 0011 (gouvernance on-chain, désormais bornée
  dans le temps), ADR 0020 / 0073 (encodage), ADR 0051 (adresses, clés)
- **Crates :** `vinx-crypto`, `vinx-core`, `vinx-state`, `vinx-node`, `vinx-wallet`,
  `vinx-desktop-core`, SDK TypeScript

---

## 1. Contexte

Certains choix se figent au premier bloc du mainnet : l'unité monétaire, le format des
adresses et des signatures, l'encodage des données signées, et qui détient le pouvoir sur la
chaîne. Aucun réseau public n'existe encore : c'est la dernière occasion de les corriger
sans casser les fonds de quelqu'un. Chaque décision ci-dessous a été revue puis tranchée
explicitement.

## 2. Décisions

### D1 — Supply : 1 milliard de VINX (au lieu de 100 milliards)

`MAX_SUPPLY_ATOMS = 1 000 000 000 × 10⁹`. Le chiffre ne change pas la valeur économique,
mais un ordre de grandeur « humain » pour 1 VINX est plus lisible pour un rail de paiement.
La courbe d'émission (ADR 0040) est inchangée : la moitié de la supply en ~20 ans.

### D2 — 9 décimales (au lieu de 18)

`DECIMALS = 9`. Largement suffisant pour tout paiement. Tout montant réaliste reste exact
en `u64` et en nombre JavaScript, ce qui simplifie les wallets et le SDK. Le type interne
reste `u128`.

### D3 — Bond validateur : 10 000 VINX

`MIN_VALIDATOR_BOND_ATOMS = 10 000 VINX` (plancher 1 000, plafond 1 000 000). L'objectif est
un ensemble de validateurs accessible aux particuliers. Le bond reste réglable dans ces
bornes (ADR 0038). Plancher de frais (0,0001 VINX) et dépôt existentiel (0,001 VINX)
inchangés.

### D4 — Frais partagés, jamais brûlés

La proposition initiale d'en brûler une partie a été **rejetée** : VinX ne détruit pas de
frais. Pour rémunérer à la fois la production et la validation :

- `FEE_PRODUCER_SHARE_BPS = 5 000` : **50 %** des frais d'un bloc vont au producteur ;
- les **50 %** restants vont à la cagnotte d'époque et sont distribués aux co-signataires
  au prorata de leur participation, par le même canal que l'émission (ADR 0028).

L'invariant `circulation + cagnotte + détruits = émis` reste exact.

### D5 — Adresses Bech32m

Bech32 ne détecte pas certaines insertions ou suppressions de caractères en fin d'adresse ;
Bech32m (BIP-350) corrige ce défaut. Le préfixe `vinx1…` est inchangé.

### D6 — Octet de type de clé ; suppression de `from`

- `PublicKey` et `VinxSignature` sont des énumérations dont le **tag borsh est l'octet de
  type de clé** : `0 = Ed25519`. Les variantes sont ajoutées uniquement en fin de liste
  (ML-DSA post-quantique, passkeys P-256, multisig natif) ; un tag n'est jamais réutilisé.
- Adresse = `BLAKE3(type ‖ clé)[..20]` : deux schémas ne peuvent pas produire la même
  adresse.
- `Transaction.from` est supprimé et `pub_key` devient obligatoire. L'expéditeur est
  **dérivé** (`Transaction::sender()`), donc il ne peut plus contredire la clé. Les
  *signing bytes* engagent la clé typée à la place de l'adresse :

  `disc(1) ‖ type(1) ‖ clé(32) ‖ to(20) ‖ amount(16 BE) ‖ fee(16 BE) ‖ nonce(8 BE) ‖
  chain_id(4 BE) ‖ expiry(0 | 1‖8) ‖ payload_len(4 BE) ‖ payload ‖ sponsor(0 | 1‖20)`

- La forme JSON d'Ed25519 est inchangée (tableau de 32 octets, signature en hexadécimal).

### D7 — Nettoyage du protocole

**a) Modules retirés.** `AnchorState` (discriminant `0x09`, jamais réutilisé), `ModuleOp`,
le registre `modules` et ses constantes sont supprimés, en cohérence avec l'ADR 0064. Le
champ dormant `foundry` est également retiré.

**b) Gouvernance « à la Linux ».** Le pouvoir d'une clé on-chain est limité dans le temps :
`ADMIN_TENURE_SECS = 365 jours` après le premier bloc. Pendant ce mandat, la clé admin (ou
le comité K-of-M) peut corriger une urgence de lancement. Ensuite, `AdminAction` et
`AnnounceUpgrade` sont refusés **pour toujours**. Aucune action (rotation, comité) ne peut
prolonger ce mandat : c'est une constante gravée, pas une valeur stockée.

La gouvernance devient alors celle d'un projet open source comme Linux : un mainteneur
(ou une petite équipe) décide de ce qui entre dans le code et publie les versions (ADR
0079), et chaque validateur choisit librement la version qu'il exécute. Le mainteneur
dirige le **logiciel**, pas la **chaîne** : il n'a aucun moyen de modifier des soldes ou
des paramètres sans l'adhésion des opérateurs. L'admission des validateurs, elle, reste
sans permission (bond via `Stake`, ADR 0038).

**c) borsh, seul format binaire.** bincode 1.x est retiré (plus maintenu, et deux formats
signifient deux sources de divergence). borsh encode les payloads signés, l'engagement de
consensus (`consensus_root`), le stockage, le snapshot et le mempool. serde ne sert plus
qu'au JSON de l'API.

## 3. Conséquences

- **Nouvelle genèse obligatoire.** Toutes les racines d'état, adresses, hachages de
  transactions et formats disque changent. Le stockage passe au schéma **v22** : toute base
  antérieure est refusée (et laissée intacte), avec une consigne explicite. Les migrations
  v6→v21, qui n'ont jamais servi à un réseau public, sont supprimées.
- Les clients externes (wallet web intégré, console admin, SDK) ont été mis à jour.
  Un client tiers doit reproduire la nouvelle disposition des *signing bytes* (§ D6).
- Après 365 jours, le seuil de frais, la taille de l'ensemble actif et le bond ne sont plus
  modifiables on-chain. Toute évolution passera par une nouvelle version du logiciel
  adoptée par les validateurs (ADR 0079).
