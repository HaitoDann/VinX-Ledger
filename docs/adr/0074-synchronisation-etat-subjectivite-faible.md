# ADR 0074 — Synchronisation d'état & subjectivité faible

- **Statut :** Partiellement implémenté 🔧 — checkpoints **Accepté 📐**, non codés
- **Date :** Septembre 2026
- **Portée :** Sécurité — modèle de confiance d'un nœud qui rejoint le réseau
- **Décideur :** VinX Labs
- **Complète :** ADR 0070 (authentification des blocs), ADR 0072 (state_root)
- **Crates :** `vinx-node` (`src/sync.rs`, `src/chain.rs`)

---

## 1. Contexte

Le snapshot-sync adopte **un état du monde entier** — tous les soldes, le set de
validateurs, la clé admin — et `Chain::new_from_snapshot` marque le bloc du snapshot
**finalisé**. C'est le chemin par lequel passe *tout* nœud rejoignant une chaîne mature :
son modèle de confiance est le modèle de confiance du réseau pour les nouveaux entrants.

Ce chemin n'avait aucune vérification utile (finding VINX-10) :

```rust
let computed_root = new_state.compute_state_root();
if computed_root != hex::decode(&snap.state_root) { return false; }
```

`snap.state_root` provient du **même pair** que `snap.state_hex`. Le contrôle démontrait
seulement que le pair savait hacher l'état qu'il venait d'envoyer. `snap.block.header.state_root`
était présent dans la même réponse, gratuit à vérifier, et jamais consulté ; aucun quorum
n'était vérifié sur le bloc déclaré irréversible. L'URL `sync_peer_rpc` n'imposait aucun
schéma : le snapshot pouvait transiter en HTTP clair.

Un opérateur de bootstrap malveillant, ou quiconque sur le chemin réseau, servait donc à un
nœud rejoignant le réseau un état dont **ses propres adresses** formaient le set de
validateurs et **sa propre clé** l'admin — et le nœud l'adoptait, marqué final.

## 2. Décision

### 2.1 Vérifications implémentées

Avant d'adopter un snapshot :

1. la racine recalculée doit égaler **`snap.block.header.state_root`** — la valeur que le
   reste du réseau voit aussi, pas une que le pair invente. `snap.state_root` n'est plus
   qu'un contre-contrôle de cohérence interne : un pair qui se contredit lui-même est refusé ;
2. la hauteur de l'en-tête doit correspondre à la hauteur annoncée, sans quoi état et chaîne
   seraient installés à des hauteurs incohérentes ;
3. le bloc du snapshot doit passer `validate_block_with_registry` contre le set de
   validateurs et le registre BLS **de l'état adopté** — un bloc ne peut pas être déclaré
   final sans quorum réel de validateurs enregistrés.

Ces trois contrôles s'appuient sur ADR 0072 : avant que le `state_root` n'engage le set de
validateurs et la clé admin, même un contrôle correct contre l'en-tête n'aurait rien protégé.

### 2.2 Transport

Le texte clair est refusé, sauf pair en loopback (développement local). La détection du
loopback **parse** l'hôte comme adresse IP au lieu de comparer un préfixe :
`127.0.0.1.evil.com` commence par `127.` mais est un nom d'hôte ordinaire contrôlé par un
attaquant. Ce cas est figé par un test — la première version du garde le laissait passer.

### 2.3 Ce que cela ne résout pas — subjectivité faible

Le nœud fait **encore confiance au set de validateurs du snapshot lui-même**. Une histoire
entièrement fabriquée mais cohérente (validateurs de l'attaquant, clés BLS enregistrées,
quorum réel sur cette branche) passe tous les contrôles ci-dessus. C'est irréductible : un
nœud sans point d'ancrage extérieur ne peut pas distinguer deux histoires internement
cohérentes.

La réponse est la **subjectivité faible**, et elle est ici *acceptée mais non codée* :

| Mesure | État |
|---|---|
| Checkpoints de confiance (hauteur + hash d'en-tête) livrés **avec le binaire**, et refus d'un snapshot qui les contredit | 📐 Accepté — à coder |
| Recoupement du snapshot auprès d'au moins deux pairs indépendants avant adoption | 📐 Accepté — à coder |
| Fenêtre de subjectivité documentée (durée au-delà de laquelle un nœud hors ligne doit re-checkpointer) | 📐 Accepté — à définir |

**C'est un prérequis de mainnet, pas de testnet** : voir ADR 0080.

## 3. Conséquences

- `sync_peer_rpc` doit être en HTTPS hors développement local. Les déploiements Docker et la
  documentation d'exploitation doivent être repris en conséquence.
- Les checkpoints introduisent un artefact versionné livré avec le binaire : leur mise à
  jour devient partie du processus de release (ADR 0079).
- Un nœud amorcé par snapshot hérite d'un `height_base` non nul. Toute méthode indexant
  `Chain::blocks` doit convertir hauteur → index (finding VINX-08) ; c'est un piège récurrent
  à vérifier en revue.

## 4. Alternatives écartées

| Option | Pourquoi écartée |
|---|---|
| Rejouer l'histoire depuis la genèse | Seule méthode sans confiance, mais impraticable sur une chaîne mature à 12 s/bloc ; le snapshot reste nécessaire |
| Faire confiance à une liste de pairs « officiels » | Recentralise le bootstrap sur VinX Labs et échoue exactement là où un checkpoint réussit : le pair peut mentir, un hash livré avec le binaire non |
| Ne rien changer et documenter le risque | Le chemin est celui de *tous* les nouveaux nœuds : c'est la surface la plus exposée, pas la plus tolérable |

## 5. Critères de validation

- [x] Un snapshot dont l'état contredit `block.header.state_root` est refusé.
- [x] Un snapshot dont le champ `state_root` contredit son propre en-tête est refusé.
- [x] Un snapshot dont le bloc n'atteint pas le quorum de son propre set est refusé.
- [x] HTTPS accepté ; HTTP public refusé ; loopback autorisé ; hôtes sosies refusés —
      `https_is_accepted_and_public_http_is_refused`,
      `loopback_http_is_allowed_for_local_development`, `lookalike_hosts_are_refused`.
- [ ] Un snapshot contredisant un checkpoint livré est refusé — **non codé**.
- [ ] Un snapshot non confirmé par un second pair indépendant est refusé — **non codé**.
