# ADR 0073 — Injectivité de la sérialisation signée (`signing_bytes`)

- **Statut :** Implémenté 🔧 — audit de sécurité de septembre 2026 (finding VINX-12)
- **Date :** Septembre 2026
- **Portée :** Sécurité des transactions — encodage du message signé · **Changement cassant le consensus**
- **Décideur :** VinX Labs
- **Révise :** ADR 0053 §3 (layout des signing_bytes), ADR 0020 (sérialisation canonique)
- **Crates :** `vinx-core` (`src/transaction.rs`), `vinx-node` (`src/rpc/ui.rs`)

---

## 1. Contexte

`Transaction::signing_bytes` concaténait `payload` **brut**, immédiatement suivi du marqueur
de sponsor (`0x00`, ou `0x01 ‖ sponsor[20]`). L'encodage n'était donc pas injectif.

Pour toute adresse de sponsor dont le dernier octet vaut `0x00` — 1 sur 256, et
**l'attaquant choisit le sponsor** — ces deux transactions produisent des octets signés
identiques :

```
A : payload = P,                    sponsor = Some(S)
    … ‖ P ‖ 0x01 ‖ S[0..20]

B : payload = P ‖ 0x01 ‖ S[0..19],  sponsor = None
    … ‖ P ‖ 0x01 ‖ S[0..19] ‖ 0x00
```

`hash()` étant `sha256(signing_bytes())`, le **txid entre également en collision** et une
signature vaut pour les deux. Or elles ne sont pas équivalentes : en A le sponsor paie les
frais, en B c'est l'émetteur. Un relais hostile transformait donc un virement sponsorisé en
un virement où l'émetteur paie des frais qu'il n'a jamais acceptés. Pour les types dont le
payload est décodé (`AdminAction` → `GovernanceAction`, `AnnounceUpgrade`), le payload
redécoupé se décode aussi différemment.

**Note sur la spécification.** ADR 0053 §3 décrivait déjà un `payload_len` dans le layout.
Le code ne l'a jamais implémenté et le layout documenté ne mentionnait pas le marqueur de
sponsor : la spec était en avance sur l'implémentation, et personne ne l'a rapproché. C'est
la leçon opératoire de ce finding — un layout consensus-critique doit être **figé par un
vecteur doré**, pas décrit en prose (ADR 0020).

## 2. Décision

### 2.1 Tout champ de longueur variable est auto-délimité

`payload` est précédé de sa longueur sur **4 octets big-endian**, écrite
**inconditionnellement** :

```
disc(1) ‖ from(20) ‖ to(20) ‖ amount(16 BE) ‖ fee(16 BE) ‖ nonce(8 BE)
       ‖ chain_id(4 BE) ‖ expiry(0x00 | 0x01‖8 BE)
       ‖ payload_len(4 BE) ‖ payload ‖ sponsor(0x00 | 0x01‖20)
```

Le préfixe est émis même pour un payload vide : ne l'émettre que pour un payload non vide
rendrait le cas vide à nouveau ambigu.

Chaque champ est désormais soit de largeur fixe, soit délimité par un drapeau, soit préfixé
par sa longueur. Aucun redécoupage de la chaîne d'octets ne peut être réinterprété comme des
frontières de champs différentes : **l'encodage est injectif**.

### 2.2 Largeur du préfixe

4 octets, pas 8 comme l'esquissait ADR 0053. `payload` est borné à 16 KiB par
`MAX_TX_PAYLOAD_BYTES` (ADR 0035 / finding VINX-13), donc `u32` couvre l'espace utile avec
six ordres de grandeur de marge, et économise 4 octets sur chaque transaction d'un rail de
paiement où la majorité des transactions ont un payload vide.

### 2.3 Les clients suivent en lockstep

La console admin embarquée (`rpc/ui.rs`) reconstruit ces octets **à la main en JavaScript**,
sur deux chemins distincts (transfert/stake et gouvernance). Les deux sont modifiés dans le
même commit : sans cela, toute signature produite par l'UI web serait rejetée. Les deux
vecteurs dorés qui figent l'équivalence Rust ↔ JS sont mis à jour.

## 3. Conséquences

- **Hard fork.** Tous les hash de transaction changent. Fait maintenant, pré-lancement.
- **Tout SDK ou portefeuille tiers** qui construit `signing_bytes` doit être repris. Il n'en
  existe pas d'externe à ce stade — c'est précisément pourquoi le moment est le bon.
- ADR 0053 §3 est corrigé : son layout ne mentionnait ni le marqueur de sponsor ni la
  vérification effective de la signature du sponsor (laquelle était par ailleurs absente du
  code, cf. finding VINX-03).

## 4. Alternatives écartées

| Option | Pourquoi écartée |
|---|---|
| Interdire les adresses de sponsor finissant par `0x00` | Corrige un symptôme (1/256) et laisse l'encodage non injectif ; toute évolution du format rouvre la classe entière |
| Passer `signing_bytes` à bincode/Borsh intégral | Réécrit le format entier, casse tous les clients et la console admin pour un gain nul : le layout manuel est lisible et reproductible par un client tiers, ce que le rail de paiement veut préserver |
| Préfixe de longueur seulement si payload non vide | Laisse le cas vide ambigu — c'est la même faille, déplacée |

## 5. Critères de validation

- [x] La collision exacte (sponsor finissant par `0x00`, payload redécoupé) produit des
      octets **et** des txid différents —
      `test_signing_bytes_payload_sponsor_boundary_is_unambiguous`.
- [x] Un payload vide et un payload `[0x00]` se distinguent —
      `test_signing_bytes_payload_length_is_committed`.
- [x] Les deux vecteurs dorés Rust ↔ JS sont à jour et verts —
      `test_signing_bytes_golden_vector`, `test_governance_signing_bytes_golden_vector`.
- [x] `cargo test --workspace` vert.
- [ ] Fuzzing de collision sur `signing_bytes` (variation type / payload / sponsor / expiry
      aux bornes) — **recommandé par l'audit ChatGPT, non réalisé** (ADR 0080).
