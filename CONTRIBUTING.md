# VinX Ledger — Guide de contribution et méthode de travail

> **Document de référence pour l'organisation du développement VinX Labs.**
> Ce fichier décrit le cycle de vie des ADRs, le processus de validation, la structure des phases et le rythme de travail.

---

## Principe fondateur

**Un changement n'existe que s'il est documenté, validé et testé — dans cet ordre.**

Pas de code d'abord. On écrit d'abord pourquoi, puis on définit comment savoir que c'est bon, puis on code. C'est le renversement central par rapport à un workflow classique, et c'est ce qui garantit la cohérence entre le code, les ADRs et le PROTOCOL_SPEC.md.

---

## 1. Les phases du projet

Le développement est organisé en phases nommées, chacune avec un objectif central clairement délimité. Une phase est terminée quand **tous les ADRs qui la composent sont à l'état Vérifié**.

| Phase | Nom | Objectif | ADRs centraux | État |
|---|---|---|---|---|
| 1 | *(à nommer)* | L1 solide : crypto, consensus BFT, émission progressive, finalité, BLS | 0051–0063, 0002–0003, 0005–0011, 0015, 0020–0022, 0026–0027, 0031, 0040, 0043, 0045, 0046 | ✅ Terminé |
| 1.5 | Durcissement & lancement | Sécurité consensus post-audit, amorçage, release, critères de lancement | 0069–0080, 0064 | 🔄 En cours |
| 2 | *(à nommer)* | PoS Algorand-style : comité VRF, pool permissionless | 0029, 0038, 0028 | 🔄 En cours |
| 3 | *(à nommer)* | Réseau public : mainnet, décentralisation à l'échelle | 0013, 0016, 0018, 0019 | 🔮 Vision |
| ❄️ | Gelé hors scope (ADR 0064) | Appchains ZK / SP1 / Celestia / ForceExit / Clearinghouse / modules bondés / tokenomics Appchains | 0050, 0034, 0048, 0049, 0010, 0024, 0039, 0041, 0044, 0047, 0023 | ❌ Abandonné |

> Les noms de phases seront ajoutés ici dès décision. Ce tableau est la feuille de route maître.

---

## 2. Le cycle de vie d'un ADR

Chaque décision technique suit obligatoirement ces cinq états, dans l'ordre :

```
Proposé → Accepté → En cours → Implémenté → Vérifié
```

### 2.1 Proposé

**Qui** : n'importe qui peut ouvrir un ADR Proposé.

**Contenu obligatoire** :
- Problème à résoudre (pourquoi maintenant ?)
- Options envisagées avec leurs compromis
- Option retenue et justification

**Ce qui est interdit à ce stade** : écrire du code lié à cet ADR.

---

### 2.2 Accepté

**Condition pour passer de Proposé → Accepté** : les **critères de validation** sont écrits dans l'ADR.

Un ADR sans critères de validation ne peut pas être Accepté. C'est la règle la plus importante du cycle.

**Les critères de validation doivent répondre à** :
- Quels tests doivent passer ?
- Quel comportement observable prouve que l'implémentation est correcte ?
- Y a-t-il un banc multi-nœuds à valider ?
- Y a-t-il une propriété de conservation à vérifier (invariant de supply, sûreté BFT, etc.) ?

**Exemple de critères bien écrits** (ADR 0029 — comité VRF) :
```
Critères de validation :
- [ ] Un nœud seul produit des blocs (sélection VRF dégénère correctement à n=1)
- [ ] Banc n=3 : les 3 leaders sont distincts sur 100 blocs consécutifs
- [ ] Banc n=3 : si 1 nœud tombe, la finalité gèle mais ne bifurque pas
- [ ] Test de propriété : aucun validateur n'est leader plus de 2× la moyenne sur 1 000 blocs
- [ ] PROTOCOL_SPEC.md §8.2 correspond à l'implémentation
```

**Exemple de critères mal écrits** (à rejeter) :
```
- [ ] Les tests passent
- [ ] Le code fonctionne
```

---

### 2.3 En cours

**Condition** : l'ADR est Accepté ET les critères de validation sont écrits.

**Convention de branche** : `adr/<NNNN>-<slug-court>`
Exemple : `adr/0029-comite-vrf`

**Règles** :
- Un seul ADR actif par branche
- Les commits doivent référencer l'ADR : `feat(adr-0029): add VRF leader selection`
- On ne merge pas sur `main` tant que l'état n'est pas Implémenté

---

### 2.4 Implémenté

**Conditions pour passer En cours → Implémenté** :
1. Tous les critères de validation sont cochés
2. `cargo test --workspace` passe sans warning
3. `cargo clippy --workspace` passe sans warning
4. **PROTOCOL_SPEC.md est mis à jour** — la constante ou la règle concernée est correcte
5. L'ADR lui-même est mis à jour (statut, date de résolution, STORAGE_VERSION si applicable)

**Ce qu'on ne fait pas** : merger sans avoir mis à jour PROTOCOL_SPEC.md. Ce fichier est la source de vérité — le code doit le rejoindre, jamais l'inverse.

---

### 2.5 Vérifié

**Condition** : l'implémentation a été validée sur un banc réel (multi-nœuds si applicable) et les critères de validation ont été exécutés, pas seulement compilés.

Un ADR peut rester Implémenté plusieurs sprints avant d'atteindre Vérifié si le banc prend du temps.

---

## 3. Règles de PROTOCOL_SPEC.md

`PROTOCOL_SPEC.md` est le contrat du protocole. Il définit les règles, les constantes et les états légitimes du système.

**Règle absolue** : si `PROTOCOL_SPEC.md` et le code se contredisent, c'est toujours le code qui a tort — jusqu'à preuve que c'est PROTOCOL_SPEC.md qui doit être mis à jour (auquel cas on ouvre un ADR).

**Mise à jour obligatoire à chaque ADR Implémenté** :
- Mettre à jour la constante correspondante (section 18)
- Mettre à jour le statut dans l'index des ADRs (section 17)
- Mettre à jour la section fonctionnelle concernée (consensus, frais, staking, etc.)

**Mise à jour interdite** : modifier PROTOCOL_SPEC.md pour "coller au code" sans ADR. Si le code a divergé, c'est un bug de process — ouvrir un ADR Proposé pour décider formellement.

---

## 4. Rythme de travail — le cycle mensuel

Le développement s'organise en cycles de 4 semaines. Ce n'est pas un sprint Agile — c'est un rythme de recherche appliquée.

```
┌─────────────────────────────────────────────────────────────────┐
│  SEMAINE 1-2 — IMPLÉMENTATION                                   │
│  → Travailler sur l'ADR "En cours" de la phase courante         │
│  → Écrire les tests unitaires en parallèle du code              │
│  → Pas de refactoring hors scope de l'ADR                       │
└─────────────────────────────────────────────────────────────────┘
           ↓
┌─────────────────────────────────────────────────────────────────┐
│  SEMAINE 3 — VALIDATION                                         │
│  → cargo test --workspace + cargo clippy                        │
│  → Banc multi-nœuds si l'ADR le requiert                        │
│  → Cocher les critères de validation dans l'ADR                 │
│  → Mettre à jour PROTOCOL_SPEC.md                               │
│  → Merger sur main si tous les critères sont verts              │
└─────────────────────────────────────────────────────────────────┘
           ↓
┌─────────────────────────────────────────────────────────────────┐
│  SEMAINE 4 — PLANIFICATION                                      │
│  → Audit des ADRs : cohérence avec PROTOCOL_SPEC.md ?           │
│  → Choisir le prochain ADR à passer Accepté                     │
│  → Écrire ses critères de validation                            │
│  → Ouvrir les prochains drafts Proposés si besoin               │
└─────────────────────────────────────────────────────────────────┘
```

---

## 5. Conventions de code

### Branches

| Type | Format | Exemple |
|---|---|---|
| ADR | `adr/<NNNN>-<slug>` | `adr/0029-comite-vrf` |
| Correction doc | `docs/<sujet>` | `docs/protocol-spec-fees` |
| Bug | `fix/<sujet>` | `fix/emission-overflow` |
| Refactoring | `refactor/<sujet>` | `refactor/worldstate-fields` |

**Jamais** de développement direct sur `main`.

### Commits

Format : `<type>(adr-<NNNN>): <description en impératif>`

```
feat(adr-0029): add ECVRF leader selection per block height
test(adr-0029): validate committee uniqueness over 1000 blocks
fix(adr-0029): correct VRF output comparison (big-endian)
docs(adr-0029): mark implemented, update PROTOCOL_SPEC §8.2
```

Types valides : `feat`, `fix`, `test`, `docs`, `refactor`, `chore`

### Tests

Chaque ADR Implémenté doit avoir au minimum :
- Des tests unitaires pour chaque nouvelle fonction
- Un test d'intégration qui vérifie le comportement observable décrit dans les critères de validation
- Un test de propriété (proptest) si l'ADR touche un invariant de supply ou une règle de consensus

---

## 6. Critères d'un bon ADR — checklist avant de passer Accepté

```
□ Le problème est clairement posé (pourquoi maintenant ?)
□ Au moins 2 options sont comparées avec leurs compromis
□ L'option retenue est justifiée
□ Les critères de validation sont écrits et vérifiables
□ Les dépendances avec d'autres ADRs sont listées
□ L'impact sur PROTOCOL_SPEC.md est identifié
□ L'impact sur les autres fichiers de documentation est identifié
```

---

## 7. Priorités courantes

Pour décider quel ADR traiter ensuite, appliquer cet ordre :

1. **Sûreté d'abord** — tout ADR qui corrige un problème de sécurité ou de conservation d'invariant prime sur tout le reste
2. **Prérequis ensuite** — si l'ADR A est prérequis de B, C, D : traiter A avant tout
3. **Phase courante** — rester dans la phase courante ; ne pas ouvrir la phase suivante avant que la courante soit Vérifiée
4. **Effort minimal** — à priorité égale, prendre le plus petit ADR d'abord (livraisons fréquentes > grands chantiers)

**ADR prérequis actuels (Phase 2)** :
- ADR 0029 (comité VRF) est prérequis de tout le reste de la Phase 2 et de la Phase 3
- ADR 0038 (pool PoS) dépend de 0029
- ADR 0028 (récompenses époque) peut commencer en parallèle de 0029

---

## 8. Documentation — règle des trois fichiers

Tout changement de protocole touche **toujours ces trois fichiers** :

| Fichier | Rôle | Mis à jour quand |
|---|---|---|
| `docs/adr/NNNN-*.md` | Décision et contexte | À chaque changement d'état de l'ADR |
| `PROTOCOL_SPEC.md` | Règles et constantes en vigueur | Quand l'ADR passe Implémenté |
| `docs/adr/README.md` | Index et état de tous les ADRs | Quand l'ADR passe Implémenté |

Les fichiers `GUIDE.md`, `GETTING_STARTED.md`, `whitepaper.md` sont mis à jour **après** — jamais comme référence primaire.

---

## 9. Définition de "terminé"

Un ADR est **terminé** (`Vérifié`) quand :

- [ ] Tous ses critères de validation sont cochés
- [ ] `cargo test --workspace` vert
- [ ] `cargo clippy --workspace` vert (zéro warning)
- [ ] Banc multi-nœuds validé (si requis par les critères)
- [ ] `PROTOCOL_SPEC.md` mis à jour
- [ ] `docs/adr/README.md` mis à jour
- [ ] L'ADR lui-même marque l'état `Vérifié` avec la date

Rien n'est "à peu près terminé". Un ADR est soit Vérifié, soit pas terminé.
