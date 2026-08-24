# ADR 0048 — ForceExit / Escape Hatch

- **Statut :** Accepté (design) — non implémenté
- **Catégorie :** Appchains / Sécurité · **Priorité :** 🔴 haute
- **Date :** Août 2026
- **Liens :** prérequis pour tout déploiement d'Appchain (ADR 0050) ; s'appuie sur le clearinghouse (ADR 0049) ; lie le bond d'Appchain (ADR 0010 étendu).

---

## 1. Contexte

Chaque Appchain tourne avec un séquenceur unique géré par son équipe (modèle V1). Ce séquenceur centralise l'ordonnancement des transactions et la génération des preuves SP1 soumises au L1. Ce modèle est simple et efficace, mais crée un point de défaillance unique pour la **liveness** :

- Le séquenceur tombe en panne → plus de nouveaux blocs Appchain → les fonds des utilisateurs sont bloqués.
- Le séquenceur censure une transaction → l'utilisateur n'a aucun recours hors-chaîne.

La **sécurité** (intégrité des fonds) reste garantie par ZK : le séquenceur ne peut pas voler les fonds même s'il est compromis. Mais la **disponibilité** des fonds est conditionnée à la liveness du séquenceur.

La solution : un mécanisme de sortie forcée directement sur le L1, indépendant du séquenceur.

---

## 2. Décision

### 2.1 Transaction ForceExit

Un nouveau type de transaction L1 `ForceExit` (code `0x0B`) permet à un utilisateur de réclamer ses fonds sur le L1 **sans passer par le séquenceur**.

**Payload :**
```
module_id: Hash32          // Identifiant de l'Appchain
user_address: Address      // Adresse L1 de l'utilisateur
claimed_balance: Amount    // Montant revendiqué
state_proof: MerkleProof   // Preuve d'inclusion dans le dernier state diff ancré
```

**Validation par le L1 :**
1. L'Appchain `module_id` est enregistrée et a un `anchor_head` valide.
2. La `state_proof` vérifie l'inclusion de `(user_address, claimed_balance)` dans le state diff ancré dans `anchor_head`.
3. Le solde n'a pas déjà été revendiqué (nullifier ou marqueur d'état).

**Effet :** le L1 crédite `claimed_balance` à `user_address` et débite le compte de séquestre de l'Appchain.

### 2.2 Séquestre obligatoire des Appchains

Pour que le `ForceExit` soit possible, l'Appchain doit maintenir un **solde de séquestre sur le L1** couvrant la totalité des fonds des utilisateurs. Ce séquestre est distinct du bond d'opérateur (ADR 0010).

- À l'enregistrement de l'Appchain : dépôt d'un séquestre initial.
- Le séquenceur met à jour le séquestre à chaque `AnchorState`.
- Si le séquestre est insuffisant pour couvrir le total des sorties en attente, le séquenceur est jailable.

### 2.3 Mécanisme de slashing pour non-inclusion

Si un `ForceExit` valide est soumis sur le L1 et que le séquenceur ne l'inclut pas dans son lot suivant sous **T_FORCE_EXIT_TIMEOUT** (valeur proposée : 10 blocs L1 ≈ 120 s) :

1. Le L1 exécute le `ForceExit` unilatéralement depuis le séquestre.
2. Le bond de l'Appchain est slashé proportionnellement au montant non inclus.
3. L'Appchain est marquée `force_exit_violated` — les nouveaux dépôts sont bloqués jusqu'à régularisation.

Cette pénalité crée un engagement économique fort : ignorer un `ForceExit` coûte plus cher que le servir.

### 2.4 État de dormance

Si une Appchain ne produit plus de preuves pendant **T_DORMANT** (proposé : 72 h ≈ 21 600 blocs L1), elle passe automatiquement en état `Dormant`. Dans cet état :
- Tout utilisateur peut soumettre un `ForceExit` immédiatement, sans délai de grâce.
- Les preuves d'appartenance sont vérifiées contre le **dernier state diff valide** ancré.
- Le séquestre est progressivement libéré vers les utilisateurs qui réclament.

---

## 3. Modèle de sécurité

| Propriété | Garantie |
|-----------|----------|
| **Fonds sûrs** | ZK proof = le séquenceur ne peut pas voler les fonds |
| **Fonds disponibles** | ForceExit = l'utilisateur peut toujours sortir |
| **Séquenceur honnête** | Slashing économique si ForceExit ignoré |
| **Séquenceur hors ligne** | État dormant → ForceExit libre |

---

## 4. Contraintes de la V1

- Le ForceExit repose sur les **state diffs** ancres sur le L1 (ADR 0050 §2.1) — si les state diffs ne couvrent pas un solde, il est non-réclamable.
- La **DA externe** (Celestia, ADR 0034 révisé) permet à l'utilisateur de reconstruire la preuve Merkle même si le séquenceur est hors ligne.
- La fréquence d'ancrage minimale (un `AnchorState` toutes les N blocs L1) doit être spécifiée comme condition d'enregistrement.

---

## 5. Conséquences

**Positif**
- Modèle de liveness non-custodial : les fonds ne sont jamais bloqués définitivement.
- Incitation économique forte pour les opérateurs d'Appchain à maintenir la liveness.
- Compatible avec un séquenceur unique (V1) sans nécessiter de décentralisation immédiate.

**Coûts / compromis**
- Ajoute un champ de séquestre à gérer pour chaque Appchain.
- Le timeout `T_FORCE_EXIT_TIMEOUT` est un paramètre de sécurité critique à calibrer.
- La vérification des state proofs Merkle sur le L1 augmente légèrement la charge de validation.
- L'état `force_exit_violated` requiert un mécanisme de résolution (gouvernance Appchain ?).

---

## 6. Notes d'implémentation

**`vinx-core` :**
- `TransactionType::ForceExit (0x0B)` avec payload structuré
- `AppchainEntry` étendu : `sequester_balance`, `force_exit_timeout_ts`, `state: AppchainState`
- `AppchainState : { Active, Dormant, ForceExitViolated }`

**`vinx-state` :**
- `apply_force_exit()` : vérifie la Merkle proof, crédite l'utilisateur, débite le séquestre
- `check_force_exit_timeout()` : appelé dans `settle_block`, dégrade l'état si timeout expiré
- Nullifier set : `force_exited: BTreeSet<(Hash32, Address)>` pour éviter double-réclamation

**`vinx-node/rpc` :**
- `GET /appchain/:id/state` : retourne l'état et le séquestre courant
- `POST /tx/submit` : accepte `ForceExit` comme tout autre type de tx

**Dépendances :** ADR 0050 (SP1 proof + AnchorState avec state diffs) ; ADR 0034 révisé (DA Celestia pour reconstruire les preuves offline).
