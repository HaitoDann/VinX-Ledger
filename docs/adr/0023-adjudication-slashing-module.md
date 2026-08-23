# ADR 0023 — Adjudication du slashing de module

- **Statut :** Proposé — 🟢 future (Phase 3)
- **Date :** Août 2026
- **Portée :** Modules — preuve et sanction on-chain d'une fraude d'opérateur de module.
- **Décideur :** VinX Labs.

---

## 1. Problème

Un opérateur de module (ADR 0010) bonde des VINX comme garantie d'honnêteté. Mais
actuellement :
- La L1 ne **vérifie pas** que l'état ancré est calculé honnêtement (garantie de disponibilité
  et d'intégrité des données laissée à l'opérateur).
- Il n'existe aucune **preuve de fraude on-chain** : si l'opérateur ancre un état invalide,
  le bond n'est pas slashé automatiquement.

L'ADR 0001 assume que ce mécanisme sera construit, mais ne le définit pas.

## 2. Options envisagées

| Option | Description | Sécurité | Complexité |
|---|---|---|---|
| A — Bond + réputation | Pas de preuve de fraude ; slash par gouvernance seulement | Niveau 1 (actuel) | Faible |
| B — Preuves de fraude optimistes | Période de challenge ; n'importe qui peut soumettre une preuve d'invalidité d'une transition | Niveau 2 | Moyenne |
| C — Preuves de validité ZK | L'ancre inclut une preuve SP1 Groth16 → la L1 vérifie la preuve sans exécuter | Niveau 3 | Haute |

## 3. Direction

**Niveau 1 (actuel)** : opérationnel via ADR 0010. Le slash par gouvernance reste le dernier
recours.

**Niveau 2 (cible Phase 3)** :
- Introduire une fenêtre de challenge (ex. 7 jours).
- Tout nœud peut soumettre une `FraudProof { module_id, old_anchor, new_anchor, witness }`.
- La L1 vérifie la `FraudProof` on-chain (possible seulement si les données sont disponibles
  — **prérequis : ADR 0034, DA Celestia**).
- Validation confirmée → slash automatique du bond opérateur.

**Niveau 3 (ADR 0050)** : intégré dans la vérification SP1 — les Appchains n'ont pas de
fenêtre de challenge, la preuve ZK élimine le besoin de fraude.

## 4. Critères de validation (à écrire avant Accepté)

- [ ] Une fraude prouvable (transition invalide avec données disponibles) déclenche un slash automatique.
- [ ] La fenêtre de challenge est configurable (gouvernance).
- [ ] L'invariant de supply est maintenu après slash (bond → pool validateurs via 0040).
- [ ] `cargo test --workspace` vert.

## 5. Dépendances

- ADR 0010 (registre de modules) : prérequis implémenté.
- ADR 0034 (DA Celestia) : les données doivent être disponibles pour prouver la fraude.
- ADR 0050 (SP1 ZK) : niveau 3 — élimine le besoin de preuve de fraude.
- Phase 3 minimum.
