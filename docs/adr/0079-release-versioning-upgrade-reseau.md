# ADR 0079 — Release, versioning et upgrade réseau

- **Statut :** Accepté 📐 — processus décidé, outillage à écrire
- **Date :** Septembre 2026
- **Portée :** Exploitation — comment une version de nœud est produite, publiée et adoptée
- **Décideur :** VinX Labs
- **Complète :** ADR 0006 (préavis d'upgrade), ADR 0061 (STORAGE_VERSION), ADR 0078 (incident)
- **Crates :** tous — processus

---

## 1. Contexte

VinX sait déjà **annoncer** un changement de protocole (`AnnounceUpgrade`, ADR 0006) et
**migrer son stockage** (`STORAGE_VERSION`, ADR 0061). Il n'existe en revanche aucun
processus décrivant comment on passe d'un commit à une version que des opérateurs
indépendants exécutent.

Le manque est devenu concret pendant l'audit : cette série contient **deux changements
cassant le consensus** — `state_root` engage désormais l'état de consensus (ADR 0072) et
`signing_bytes` préfixe le payload (ADR 0073). Les deux ont été appliqués sans hauteur
d'activation ni migration, ce qui n'est acceptable que parce que la chaîne est en
`0.1.0-alpha.1` **sans réseau public**. Après le lancement, la même modification exigerait
un protocole d'activation coordonné. Rien n'écrit aujourd'hui cette distinction.

## 2. Décision

### 2.1 Trois axes de version, distincts

| Version | Ce qu'elle décrit | Où elle vit |
|---|---|---|
| **Version logicielle** | Le binaire (SemVer) | `Cargo.toml` |
| **Version de protocole** | Les règles de consensus | `ProtocolVersion` on-chain (ADR 0006) |
| **Version de stockage** | Le format sur disque | `STORAGE_VERSION` (ADR 0061) |

Elles évoluent **indépendamment**. Un correctif de performance n'incrémente que la première ;
une nouvelle règle de validation incrémente la deuxième ; un champ d'état ajouté incrémente
la troisième. Confondre les trois est la source classique de nœuds qui divergent en silence.

### 2.2 Classification d'un changement

Tout changement est classé **avant** la revue :

| Classe | Critère | Exigence |
|---|---|---|
| **Consensus-breaking** | Deux nœuds de versions différentes peuvent diverger sur la validité d'un bloc ou sur `state_root` | Hauteur d'activation + `AnnounceUpgrade` + préavis ADR 0006 |
| **Compatible réseau** | Change le format de message P2P mais reste interopérable (champs `serde(default)`) | Fenêtre de double support documentée |
| **Local au nœud** | Performances, journalisation, RPC, outillage | Release logicielle simple |

En cas de doute, un changement est **consensus-breaking**. `state_root` (ADR 0072) et
`signing_bytes` (ADR 0073) sont les exemples de référence.

### 2.3 Activation par hauteur, jamais par version de binaire

Une règle de consensus nouvelle s'active à une **hauteur** inscrite on-chain, pas parce
qu'un opérateur a mis à jour. Un nœud à jour applique l'ancienne règle jusqu'à la hauteur
d'activation et la nouvelle après. C'est ce qui permet à des opérateurs de mettre à jour à
des moments différents sans casser le consensus — le contraire même de ce qui a été fait
pour 0072/0073, acceptable uniquement pré-lancement.

### 2.4 Contenu d'une release

Une version publiée porte : les notes de version avec la classe de changement, les hachages
des artefacts, les checkpoints de subjectivité faible à jour (ADR 0074), la version de
stockage cible et le chemin de migration, et un `CHANGELOG.md` distinguant explicitement
les correctifs de sécurité.

### 2.5 Chemin distinct pour les correctifs de sécurité

Un correctif critique ne suit pas le cycle normal (ADR 0078 §2.3) : commit préparé en
privé, validateurs prévenus hors-bande, publication après déploiement majoritaire. Un
message de commit trop explicite avant déploiement **est** une divulgation.

Cette contrainte est en tension directe avec les messages de commit détaillés produits par
l'audit — appropriés pour une chaîne pré-lancement sans valeur à protéger, à proscrire
après le mainnet.

### 2.6 Portes de qualité

Aucune release sans : `cargo fmt --check`, `cargo clippy` sans erreur, `cargo test
--workspace` vert, **chaque commit compilant et passant les tests isolément** (l'historique
doit rester bissectable — discipline appliquée à cette série), et une migration de stockage
testée depuis la version publiée précédente sur un jeu de données réel.

## 3. Conséquences

- La CI doit vérifier la bissectabilité, pas seulement l'état de la branche.
- La classification §2.2 devient une case obligatoire de la revue.
- Un mécanisme de hauteur d'activation par règle doit exister **avant** le mainnet :
  `ProtocolVersion` et `pending_upgrade` en fournissent le socle, mais aucune règle de
  consensus n'y est aujourd'hui conditionnée.
- Les checkpoints (ADR 0074) devenant un artefact de release, leur génération doit être
  outillée et reproductible.

## 4. Alternatives écartées

| Option | Pourquoi écartée |
|---|---|
| Une seule version pour tout | Force une mise à jour de tous les nœuds pour un changement de journalisation, et masque les vrais changements de consensus |
| Activation « quand la majorité a mis à jour » | Rend l'activation dépendante d'une mesure non vérifiable on-chain ; une hauteur est déterministe |
| Publier les correctifs de sécurité comme les autres | Arme l'attaquant pendant la fenêtre de déploiement (ADR 0078) |

## 5. Critères de validation

- [ ] Classification §2.2 obligatoire dans le modèle de PR.
- [ ] CI vérifiant que chaque commit compile et passe les tests.
- [ ] Au moins une règle de consensus conditionnée à une hauteur d'activation, testée sur
      les deux branches (avant/après activation).
- [ ] Migration de stockage testée depuis la release précédente sur données réelles.
- [ ] Procédure de release de sécurité exécutée une fois à blanc (ADR 0078).
