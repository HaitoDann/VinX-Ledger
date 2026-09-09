# ADR 0070 — Authentification cryptographique du proposeur & liaison des co-signatures au registre BLS

- **Statut :** Implémenté 🔧 — audit de sécurité de septembre 2026 (findings VINX-01, VINX-02, VINX-05)
- **Date :** Septembre 2026
- **Portée :** Consensus — autorité d'un bloc et comptage des co-signatures
- **Décideur :** VinX Labs
- **Complète :** ADR 0029 (comité BLS), ADR 0046 (agrégation BLS), ADR 0063 (PoA Threshold)
- **Crates :** `vinx-core` (`src/block.rs`), `vinx-node` (`src/consensus.rs`, `src/chain.rs`, `src/p2p/mod.rs`)

---

## 1. Contexte

L'audit contradictoire de septembre 2026 a démontré deux défauts qui, combinés, retiraient
au consensus toute autorité vérifiable. Les deux sont reproduits par des tests dans
`crates/vinx-node/tests/audit_regression_node.rs`.

### 1.1 Le quorum BLS était falsifiable

`Block::bls_signer_count_from_bitmap` — la primitive présentée comme le contrôle « lié au
registre » — commençait par un repli :

```rust
if self.bls_bitmap.is_empty() {
    return self.bls_signer_count();   // clés fournies par le bloc lui-même
}
```

`bls_signer_count()` reconstruit ses signataires depuis `bls_cosigner_pks`, un champ que le
bloc transporte. Un attaquant générait `quorum` clés BLS, signait le hash d'en-tête,
publiait les clés publiques et **omettait simplement le bitmap**. Démontré sur un set de 7
validateurs (quorum 5) dont le registre était entièrement vide : `validate_block_with_registry`
retournait `Ok` et `is_finalized` retournait `true`.

Le repli était justifié en commentaire comme compatibilité avec les blocs pré-ADR-0029.
C'était un **mode de dégradation choisi par l'attaquant** : omettre un champ désactivait le
contrôle.

### 1.2 Aucun bloc n'était authentifié sur le chemin P2P

Le bras `NewBlock` vérifiait le chaînage, les bornes de timestamp, l'appartenance du
proposeur au set, les signatures de transactions, la transition d'état et le `state_root` —
et ne lisait **jamais** `bls_aggregate`. Ni `validate_block` ni
`validate_block_with_registry` n'était appelé depuis `p2p/mod.rs`.

Or `BlockHeader` ne porte aucune signature de proposeur et le type `BlockSignature` n'est
utilisé nulle part en production : l'agrégat était la seule preuve d'autorité, et personne
ne la regardait. `header.validator` étant une adresse publique, n'importe quel pair sans
clé pouvait se déclarer proposeur.

Le chemin `CompactBlock` était pire : son format de message ne transportait que `header` et
`tx_hashes`, et la reconstruction fixait `bls_aggregate: None, bls_cosigner_pks: vec![],
bls_bitmap: vec![]` avant de réinjecter en `NewBlock`. **Un bloc sans aucune signature
était appliqué.**

### 1.3 Le poids de fork-choice était choisi par l'auteur du bloc

`consensus::signer_weight` et `Chain::advance_finality` utilisaient le même compteur non
vérifié. Un bloc rembourré de 50 clés auto-générées battait un bloc réellement co-signé à
chaque hauteur contestée, puis finalisait.

## 2. Décision

### 2.1 Le registre on-chain est la seule autorité de comptage

Le repli est supprimé. `bls_signer_count_from_bitmap` est l'unique chemin de comptage
sécurisé :

- bitmap vide **avec** agrégat → `Err` (signatures attribuées à aucun index de validateur,
  donc malformé) ;
- bitmap vide **sans** agrégat → `Ok(0)` ;
- sinon, chaque bit positionné doit correspondre à une clé G1 enregistrée dans
  `validator_pool`, et l'agrégat est vérifié contre exactement ces clés.

`sign_block_bls` a toujours positionné le bitmap : exiger sa présence ne coûte rien à un
producteur légitime.

Les clés G1 dupliquées parmi les signataires reconstruits sont rejetées — une signature ne
peut pas compter sous deux bits (voir ADR 0075 §4 pour la liaison d'identité, encore
ouverte).

`bls_signer_count` est renommé **`bls_signer_count_unverified`** et documenté comme valeur
d'affichage uniquement, afin qu'aucune décision de sécurité ne l'atteigne par accident.
`consensus::validate_block`, le jumeau non enregistré de `validate_block_with_registry`,
est supprimé : **il n'existe qu'une seule définition de la validité d'un bloc.**

### 2.2 Le proposeur doit prouver qu'il a écrit le bloc

Nouvelle primitive `consensus::verify_proposer_authenticated(block, validator_set,
indexed_bls_pks)` :

1. le proposeur appartient au set (index connu) ;
2. **son bit est positionné** dans `bls_bitmap` ;
3. l'agrégat vérifie contre les clés enregistrées des bits positionnés.

Une clé n'entre au registre qu'après une Proof-of-Possession vérifiée (ADR 0046), donc
l'agrégat n'est pas forgeable sans la clé secrète du proposeur. Toute altération de
l'en-tête change `header.hash()` et fait échouer la vérification.

Appelée sur **tous** les chemins d'ingestion, avant tout travail d'état : `NewBlock`,
`SyncResponse`, et les deux sites de reconstruction `CompactBlock`.

### 2.3 Le quorum n'est délibérément pas exigé à l'ingestion

Un bloc fraîchement gossipé ne porte que la co-signature de son producteur et accumule les
autres ensuite via le chemin BLS. Exiger le quorum à l'ingestion figerait la chaîne.
`verify_proposer_authenticated` répond à une question différente — *qui a écrit ce bloc* —
qui n'avait aucune réponse. Le quorum reste exigé là où il a du sens :
`validate_block_with_registry` et la finalité.

### 2.4 Le format `CompactBlock` transporte la preuve

Le message gagne `bls_aggregate: Option<Vec<u8>>` et `bls_bitmap: Vec<u8>`
(`#[serde(default)]`), et la réassemblage les préserve, de sorte qu'un bloc reconstruit est
authentifié comme n'importe quel autre.

### 2.5 Le registre irrigue toutes les décisions de consensus

`is_finalized`, `advance_finality`, `signer_weight`, `more_canonical`, `canonical_head`,
`canonical_choice`, `would_reorg_at` et `BlockResponse::from_block` prennent désormais
`indexed_bls_pks`, construit par `WorldState::indexed_bls_keys`. Le poids de fork-choice est
par construction borné par la taille du set.

## 3. Conséquences

- **Prérequis opérationnel :** un validateur dont la clé BLS n'est pas enregistrée
  on-chain ne peut pas être authentifié, donc ses blocs sont refusés. C'est la seule
  posture sûre possible. Voir ADR 0075 pour l'enrôlement et la genèse.
- **Changement de format P2P :** `CompactBlock` gagne deux champs. Un message ancien
  décode avec un bitmap vide et est donc refusé — ce qui est le comportement voulu.
- La finalité est désormais indissociable du registre : un `WorldState` est nécessaire pour
  évaluer la finalité, ce qui contraint l'ordre des verrous côté nœud.

## 4. Alternatives écartées

| Option | Pourquoi écartée |
|---|---|
| Garder le repli borné par une hauteur d'activation | Aucune chaîne en production ne porte de blocs pré-ADR-0029 ; le repli n'aurait servi qu'à l'attaquant |
| Ajouter une signature Ed25519 du proposeur dans `BlockHeader` | Redondant : la co-signature BLS du proposeur prouve déjà la paternité, et un second schéma élargit la surface pour rien |
| Exiger le quorum dès l'ingestion | Fige la chaîne : un bloc neuf ne porte qu'une signature |

## 5. Critères de validation

- [x] Un quorum de clés non enregistrées est rejeté, registre vide comme registre plein —
      `forged_quorum_via_empty_bitmap_is_rejected`.
- [x] Le fork-choice préfère 2 co-signataires enregistrés à 50 clés forgées, et le bloc
      forgé ne finalise pas — `fork_choice_ignores_unregistered_cosigners`.
- [x] Un bloc non signé, rembourré de clés étrangères, ou co-signé au mauvais index est
      refusé ; un bloc réellement signé est accepté — `proposer_impersonation_is_rejected`.
- [x] Une même clé G1 sous deux bits ne compte pas deux fois —
      `test_duplicate_registered_key_across_slots_rejected`.
- [x] Le banc n=3 (round-robin, finalité prefix-closed, sûreté sous quorum, convergence du
      fork-choice) reste vert sur le chemin registre — `bench_n3.rs`.
- [ ] Banc adversarial multi-nœuds avec partitions réseau — **non réalisé**, voir ADR 0080.
