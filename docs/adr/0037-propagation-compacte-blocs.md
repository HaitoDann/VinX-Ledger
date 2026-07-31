# ADR 0037 — Propagation compacte des blocs

- **Statut :** Proposé
- **Catégorie :** Réseau P2P · **Priorité :** 🟢 future
- **Date :** Juillet 2026
- **Liens :** complète le durcissement P2P (ADR 0022) et la mise à l'échelle du consensus
  (ADR 0029) ; interagit avec les bornes de ressources (ADR 0035).

## Contexte

À la production, un bloc est diffusé **plein** via gossip (`NewBlock(Block)`) — en-tête +
**toutes les transactions** + signatures. Or ces transactions ont déjà transité **une fois**
sur le réseau via `NewTransaction` et sont **déjà dans le mempool** des pairs (staging pour la
vérif parallèle différée). Rediffuser les tx complètes dans le bloc **double la bande passante**
utile et gonfle la latence de propagation — d'autant plus que `max_block_txs = 10_000` peut
produire des blocs de plusieurs Mo.

Ce coût est tolérable à petite échelle, mais devient un **goulot** quand blocs pleins + beaucoup
de validateurs se combinent (ADR 0029). C'est exactement ce que résolvent les *compact blocks*
de Bitcoin (BIP 152) et le gossip d'Ethereum.

## Décision proposée

Diffuser une **annonce de bloc compacte** au lieu du bloc plein, et laisser les pairs
**reconstruire** le bloc depuis leur mempool.

1. **`CompactBlock`** : en-tête + signatures + la **liste ordonnée des hachages de tx** (ou de
   *short-ids* dérivés, à la BIP 152), pas les tx complètes.
2. **Reconstruction locale** : le pair retrouve chaque tx dans son mempool par hash et
   reconstitue le bloc. Comme le mempool contient déjà les tx (staging), la reconstruction est
   quasi-gratuite dans le cas nominal.
3. **`GetBlockTxs` / repli** : pour les tx **manquantes** (pas encore vues, ou évincées), le
   pair demande **seulement celles-là** à l'émetteur (`GetBlockTxs { height, indices }` →
   `BlockTxs`). En dernier recours, repli sur le `NewBlock` plein existant.
4. **Compatibilité** : `NewBlock` plein reste supporté (pairs anciens, sync historique via
   `SyncResponse`) ; `CompactBlock` est un **nouveau message P2P** additif.

### Déterminisme & sécurité

- La reconstruction doit aboutir au bloc **exact** (mêmes octets, même `BlockHeader::hash`) —
  l'ordre des tx est fixé par la liste de hachages ; toute divergence est détectée par le hash
  d'en-tête et l'invariant de masse (déjà vérifiés à la réception).
- **Short-ids** (si utilisés) : risque de collision à borner (salage par hash d'en-tête, façon
  BIP 152) ; sinon, hachages complets (32 o) — plus gros mais sans collision.
- Reste **sous les bornes** de l'ADR 0022 (taille de message) et 0035 (poids de bloc) — la
  vérif parallèle des signatures (ADR 0015) s'applique après reconstruction, inchangée.

## Conséquences

**Positif**
- Réduit fortement la **bande passante** et la **latence** de propagation des blocs (les tx ne
  transitent plus deux fois) → meilleure liveness et passage à l'échelle (synergie ADR 0029).
- Additif et rétro-compatible ; le repli garantit la robustesse quand le mempool diverge.

**Coûts / pièges**
- **Round-trip supplémentaire** quand des tx manquent (mempool désynchronisé, tx just-in-time) →
  à mesurer ; le repli plein borne le pire cas.
- Complexité protocolaire (nouveaux messages, reconstruction, gestion des manquants) — surface
  de bugs et de test réseau accrue.
- Collisions de short-ids si mal salées → préférer les hachages complets tant que la bande
  passante le permet, short-ids seulement si le profilage le justifie.
- Bénéfice **faible tant que le débit reste modeste** → priorité 🟢 : à faire quand blocs pleins
  + N validateurs deviennent réellement le goulot (avec/après ADR 0029).

## Alternatives écartées

- **Garder `NewBlock` plein uniquement** : rejeté à terme — double la bande passante utile,
  goulot à l'échelle.
- **Ne diffuser que l'en-tête et forcer un pull complet** : rejeté — trop de round-trips ; la
  liste de hachages permet la reconstruction en un coup dans le cas nominal.
- **Erasure-coding / diffusion par fragments** : hors périmètre ici (relève d'une couche DA /
  d'un consensus à très grande échelle) — les compact blocks sont le gain simple et éprouvé.

## Notes d'implémentation

- `vinx-node/p2p/messages.rs` : `CompactBlock { header, signatures, tx_hashes }`,
  `GetBlockTxs { height, indices }`, `BlockTxs { txs }` — encodage canonique + bornes 0022.
- `vinx-node/p2p` : émettre `CompactBlock` en production ; à la réception, reconstruire depuis
  le mempool, demander les manquants, replier sur `NewBlock` plein si besoin ; vérif inchangée
  après reconstruction (0015 + invariant de masse).
- Tests : reconstruction nominale (mempool complet), manquants → `GetBlockTxs`, repli plein,
  hash d'en-tête identique, robustesse aux short-ids (si retenus).
- Dépendances : additif sur ADR 0022 ; gain surtout pertinent avec ADR 0029 (grand N).
