# ADR 0080 — Critères de lancement : testnet public, puis mainnet

- **Statut :** Accepté 📐 — critères décidés, aucun n'est encore atteint
- **Date :** Septembre 2026
- **Portée :** Produit & sécurité — ce qui doit être vrai avant d'exposer VinX à un réseau public, puis à de la valeur réelle
- **Décideur :** VinX Labs
- **Complète :** tous les ADR de lancement (0070–0079)
- **Crates :** aucun — porte de sortie

---

## 1. Contexte

VinX est fonctionnellement complet pour un rail de paiement (ADR 0064) : consensus, finalité,
émission, P2P, sync, explorateur, portefeuille. La tentation naturelle est de considérer
« ça tourne » comme « c'est prêt ».

L'audit contradictoire de septembre 2026 a montré l'écart. Sur le commit `8774806`, un pair
**sans aucune clé** pouvait fabriquer des blocs acceptés par tout le réseau, forger un
quorum BLS avec des clés qu'il générait lui-même, et **vider n'importe quel compte** en le
désignant comme `sponsor`. Ces failles n'étaient pas théoriques : chacune est reproduite par
un test qui passait sur le code d'alors.

Ce n'est pas un reproche à l'ingénierie — c'est la démonstration que « le réseau produit des
blocs » ne dit rien sur « le réseau résiste à un adversaire ». Cet ADR écrit la différence
sous forme de conditions vérifiables, pour que la décision de lancer ne soit pas une
impression.

**Règle :** un critère non coché bloque. Il n'y a pas de « on verra en production ».

## 2. Porte 1 — Testnet public

Réseau ouvert, jetons sans valeur, plusieurs opérateurs indépendants.

### 2.1 Sécurité

- [x] Toutes les vulnérabilités **CONFIRMED** de l'audit de septembre 2026 sont corrigées,
      chacune avec un test de régression qui échoue si le correctif est retiré.
- [x] Une seule définition de la validité cryptographique d'une transaction
      (`verify_tx_signature_pure`) et une seule de la validité d'un bloc
      (`validate_block_with_registry`).
- [x] Tout bloc entrant est authentifié contre le registre BLS on-chain (ADR 0070).
- [x] Le `state_root` engage l'état de consensus (ADR 0072).
- [x] La sérialisation signée est injective (ADR 0073).
- [x] Une signature par validateur et par hauteur, verrou durable (ADR 0071).
- [ ] **ADR 0069 (BLAKE3) tranché.** Il se déclare « à implémenter avant genesis block 0 »
      et change **tous** les hachages du protocole, mais `grep -rn blake3 crates/` ne
      retourne rien : le code hache en SHA-256. Soit il est implémenté avant la genèse du
      testnet, soit il est explicitement repoussé derrière une hauteur d'activation
      (ADR 0079 §2.3). Le laisser en suspens garantit un hard fork non planifié.
      Les ADR 0065–0068 (performance) sont dans le même état « Décidé, non implémenté » —
      sans impact consensus, donc non bloquants.
- [ ] **Contre-audit externe** des correctifs traité : les prompts existent
      (`audit/post-fix/`), les retours ne sont pas revenus. Tout nouveau finding CONFIRMED
      est corrigé et testé avant la porte.

### 2.2 Banc adversarial multi-nœuds — **le manque le plus important**

Tout ce qui précède est validé **en un seul processus**. Les propriétés protégées sont des
propriétés **de réseau**. Un banc à 4 ou 7 validateurs, avec partitions contrôlées et un
nœud attaquant, doit vérifier automatiquement :

- [ ] deux blocs finalisés à la même hauteur n'ont jamais des hachages différents ;
- [ ] un validateur ne signe jamais deux hachages à la même hauteur ;
- [ ] aucun bloc ne devient final sans signatures de clés de validateurs réelles ;
- [ ] tous les nœuds recevant un bloc s'accordent sur la validité cryptographique de ses
      transactions ;
- [ ] le fork-choice ne peut jamais remplacer un bloc finalisé ;
- [ ] une entrée P2P malformée ne modifie jamais l'état de consensus ;
- [ ] un producteur ne peut jamais valider localement ce qu'un validateur honnête rejette.

Ces invariants sont ceux proposés par l'audit ChatGPT ; ils sont adoptés tels quels.

### 2.3 Amorçage et exploitation

- [x] Un réseau multi-nœuds peut démarrer : clés BLS à la genèse, enrôlement automatique,
      clé BLS persistée (ADR 0075).
- [x] Le matériel de clé est écrit en `0600` (ADR 0076).
- [ ] Un réseau à 3 validateurs indépendants démarre depuis une spec partagée et **tous**
      deviennent authentifiables sans intervention manuelle.
- [ ] Un validateur redémarre et ses blocs restent acceptés par ses pairs.
- [ ] Documentation d'exploitation : fichiers à sauvegarder, interdiction de deux instances
      partageant une clé BLS, exigence HTTPS pour `sync_peer_rpc`.

### 2.4 Divulgation

- [ ] `SECURITY.md` publié avec contact, clé publique, périmètre et délais (ADR 0078).

## 3. Porte 2 — Mainnet

Tout ce qui précède, **plus** :

### 3.1 Sécurité du protocole

- [ ] Subjectivité faible : checkpoints livrés avec le binaire, recoupement multi-pairs
      (ADR 0074 §2.3).
- [ ] Clé BLS exigée au bonding, PoP liée à `adresse ‖ chain_id` (ADR 0075 §3.1–3.2).
- [ ] Vecteur doré figeant l'encodage `consensus_root` (ADR 0072).
- [ ] Fuzzing de collision sur `signing_bytes` (ADR 0073).
- [ ] **Audit externe humain** — les audits IA de septembre 2026 ont trouvé des failles
      réelles, mais l'un des trois rapports citait six fichiers inexistants : ils ne
      remplacent pas une revue humaine spécialisée en consensus.

### 3.2 Processus

- [ ] Activation par hauteur pour toute règle de consensus (ADR 0079 §2.3) — aujourd'hui
      aucune règle n'y est conditionnée.
- [ ] Migration de stockage testée depuis la release précédente sur données réelles.
- [ ] Historique bissectable vérifié en CI.
- [ ] Rôles d'incident désignés, liste de contact validateurs hors-bande testée, exercice
      d'incident réalisé (ADR 0078).

### 3.3 Produit

- [ ] Les trois états de paiement (`pending` / `included` / `final`) exposés par l'API, le
      SDK et l'explorateur, sans possibilité de confondre `included` et `final` (ADR 0077).
- [ ] Documentation d'intégration marchand, limites comprises.

### 3.4 Décentralisation

- [ ] Set de validateurs suffisant pour que l'hypothèse BFT signifie quelque chose : un set
      contrôlé par une seule entité rend toute la sûreté de §2.2 décorative.
- [ ] Clé admin sous multisig K-of-M (ADR 0011), pas une clé unique.
- [ ] Durée minimale de testnet sans incident critique, à fixer explicitement — un nombre
      décidé **avant** d'en avoir besoin.

## 4. Ce que cet ADR ne prétend pas

Aucune liste ne rend un système sûr, et cocher ces cases ne signifiera pas « VinX est
sécurisé ». Elle réduit un risque et rend l'état de ce risque **vérifiable** — ce qui permet
au moins de décider en connaissance de cause plutôt que par optimisme.

L'exigence de fond reste celle de la méthode d'audit : pour chaque problème de sécurité,
`finding → preuve → test → correction → contre-audit → non-régression` doit être traçable.

## 5. Critères de validation

Cet ADR **est** la liste de critères. Il est satisfait quand toutes les cases de la porte
visée sont cochées, chacune adossée à un test, un artefact publié ou une procédure exécutée
— jamais à une opinion.
