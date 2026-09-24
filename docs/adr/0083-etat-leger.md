# ADR 0083 — État léger : arbre de Merkle creux, élagage, snapshots, reçus

- **Statut :** Accepté ✅ — implémenté
- **Date :** Septembre 2026
- **Portée :** Protocole (racine d'état, racine des transactions) et nœud (stockage, sync)
- **Décideur :** VinX Labs (mainteneur)
- **Modifie :** ADR 0014 (preuves de solde), ADR 0074 (snapshots), ADR 0005 (horloge MTP après snapshot)
- **Crates :** `vinx-crypto`, `vinx-state`, `vinx-node`, `vinx-wallet`, SDK TypeScript

---

## 1. Contexte

Un rail de paiement n'a besoin, pour valider, que des **soldes courants**. L'historique
complet n'est utile qu'aux explorateurs, et la preuve d'un paiement appartient d'abord
à ceux qui l'ont fait. Le but est un nœud léger à faire tourner, dont le coût ne croît
pas avec l'âge de la chaîne, sans perdre la vérifiabilité.

## 2. Décisions

### L1 — Arbre de Merkle creux pour les comptes

- **Structure.** Arbre binaire compressé sur des clés de 256 bits : `account_key =
  H("VINX_ACCOUNT_KEY" ‖ adresse)`. Un sous-arbre vide vaut zéro, et un sous-arbre à une
  seule feuille est remplacé par cette feuille. C'est la sémantique de Diem/Aptos ;
  Jellyfish stocke le même arbre en 16-aire sur disque. Les hachages sont séparés par
  domaine : `VINX_SMT_LEAF` et `VINX_SMT_NODE`.
- **Ce qu'il remplace.** L'ancien arbre trié réindexait toutes les feuilles, en O(n), à
  chaque nouveau compte. Le nouveau arbre coûte O(log n) par modification, et sa racine
  ne dépend pas de l'ordre d'insertion.
- **Preuves.** Il fournit des preuves d'**inclusion** et de **non-inclusion**.
  `GET /account/:addr/proof` prouve désormais un compte contre le `state_root` du bloc,
  via `state_root = H(DST ‖ accounts_root ‖ consensus_root)`. L'ancienne preuve visait
  une racine qui n'était pas celle du header.
- **SDK.** Il vérifie la preuve en BLAKE3 (`verifyAccountProof`). L'ancien vérificateur
  en SHA-256 était périmé depuis l'ADR 0069. Des vecteurs dorés sont générés par Rust et
  vérifiés côté Rust comme côté TypeScript.

### L2 — Copies d'état en O(1)

- **Principe.** Chaque proposition et chaque validation travaille sur une copie de
  l'état. Les comptes vivent maintenant dans une map persistante (`imbl::OrdMap`) et
  l'arbre partage ses nœuds. Une copie ne paie donc que les comptes qu'elle modifie.
- **Compatibilité.** Les encodages borsh et serde sont identiques à ceux d'une
  `BTreeMap` : le format disque ne change pas.
- **Mesure** (`tests/scale_bench.rs`, 200 000 comptes, 1 000 mises à jour par bloc) :
  un bloc qui crée 100 nouveaux comptes passe de **92 ms à 16 ms**. Le coût ne croît plus
  avec le nombre total de comptes.
- **Hors périmètre.** L'état reste chargé en mémoire et persisté compte par compte sur
  disque (redb). Un état entièrement paginé depuis le disque n'est pas utile à cette
  échelle : environ 150 octets par compte, soit ~1,5 Go pour 10 M de comptes. Il pourra
  être ajouté derrière la même interface.

### L3 — Élagage par fenêtre de 30 jours

- **Règle.** `BLOCK_RETENTION_SECS = 30 jours`, durée supérieure à l'unbonding prévu.
  Au-delà, les blocs **entiers** (en-tête, transactions, certificat) sont supprimés, en
  mémoire comme sur disque. Avant, seules les transactions étaient retirées et les
  en-têtes s'accumulaient sans fin.
- **Garde-fou.** L'élagage garde toujours les 11 derniers blocs, qui forment la fenêtre
  de l'horloge protocole (MTP).

### L4 — Snapshots engagés par le quorum

- **Vérifications.** Un snapshot est vérifié de trois façons :
  - l'état doit redonner le `state_root` du header ;
  - le bloc doit être signé par plus de 2/3 de son set de votants ;
  - les checkpoints de l'ADR 0074 s'appliquent.
- **Bug corrigé.** Un nœud démarré par snapshot n'avait qu'un bloc. Son horloge
  protocole (médiane des 11 derniers blocs) différait donc de celle de ses pairs, et il
  rejetait des blocs valides. Le snapshot transporte maintenant les 10 blocs précédents,
  authentifiés par chaînage de hash jusqu'au bloc certifié.

### L5 — Mode archive

`--archive` (ou `archive = true` dans la configuration) désactive l'élagage. Ce mode est
destiné aux explorateurs et aux services d'historique.

### L6 — Reçus de paiement

- **Racine des transactions.** `receipts_root` devient un arbre de Merkle sur les hachages
  des transactions. Il est séparé par domaine (`VINX_TX_LEAF` / `VINX_TX_NODE`), et un
  nœud impair est promu, jamais dupliqué : c'est la parade à la malléabilité
  CVE-2012-2459 de Bitcoin. L'ancienne racine était un hachage plat : prouver un paiement
  exigeait de transmettre tout le bloc.
- **Endpoint.** `GET /tx/:hash/proof` renvoie le reçu : index, voisins, header, hash du
  bloc, certificat de quorum et transaction.
- **Wallet.** `vinx-wallet receipt <hash>` récupère le reçu, le vérifie et l'enregistre ;
  `vinx-wallet verify-receipt <fichier>` le vérifie hors-ligne.
- **SDK.** Il fournit `paymentReceipt` et `verifyPaymentReceipt`, ainsi que `headerHash`.
- **Validité.** Le reçu reste vérifiable après l'élagage du bloc.

## 3. Conséquences

- **Rupture de protocole.** La racine d'état et `receipts_root` changent : il faut une
  nouvelle genèse. C'est acceptable avant le lancement.
- **Nœud trop en retard.** Un nœud en retard de plus de 30 jours doit repartir d'un
  snapshot. C'est automatique au démarrage au-delà de 500 blocs de retard. Pendant
  l'exécution, les pairs ne peuvent plus lui servir les blocs élagués.
- **Limites connues :**
  - le SDK ne vérifie pas les signatures BLS du certificat : la confiance dans le bloc
    repose sur le certificat vérifié par un nœud, ou sur la comparaison du `block_hash`
    avec un nœud de confiance ;
  - le certificat est sérialisé en JSON sous forme de tableaux d'octets.

## 4. Vérification

- **Tests unitaires :**
  - SMT : insertion et suppression, indépendance de l'ordre, construction en bloc,
    preuves ;
  - arbre des transactions : toutes les tailles de 1 à 39, malléabilité ;
  - élagage, équivalence entre chaîne issue d'un snapshot et chaîne complète, ancêtre
    falsifié ;
  - suppression sur disque suivie d'un rechargement.
- **Tests d'intégration :**
  - un reçu de paiement contrôlé contre le certificat de quorum ;
  - SDK : 29 tests, contre des vecteurs produits par le nœud ;
  - wallet : vérification d'un vrai reçu de nœud.
- **Vrai réseau** (`scripts/bench-n4.sh`) : un paiement réel envoyé à node1, dont le
  reçu est obtenu auprès de node3 puis vérifié hors-ligne, et un solde du destinataire
  identique sur node2. ✅
