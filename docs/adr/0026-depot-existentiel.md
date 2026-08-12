# ADR 0026 — Dépôt existentiel (anti-bloat de l'état)

- **Statut :** Accepté — ✅ implémenté (consensus-breaking : la règle entre dans la
  fonction de transition d'état)
- **Catégorie :** Données, état & scaling · **Priorité :** 🔴 haute
- **Date :** Juillet 2026
- **Liens :** complète l'ADR 0013 (cycle de vie de l'état) ; motivé par la
  simulation de stockage (terme dominant à long terme = le nombre de comptes).

## Contexte

L'état (`WorldState.accounts`, une `BTreeMap<Address, Account>`) **n'est jamais
élagué** : la chaîne est prunée (en-têtes seuls après `BLOCK_RETENTION_COUNT`),
mais l'état, lui, croît de façon monotone. Chaque compte pèse **60 octets pour
toujours** (`addr20 + balance16 + nonce8 + staked16`).

Deux chemins créent un compte **sans aucun coût plancher** :

- `apply_transfer` : `self.accounts.entry(tx.to).or_insert_with(|| Account::new(tx.to))`
  — n'importe quel transfert vers une adresse neuve la matérialise, **même pour
  1 atom**.
- `credit` : idem via `or_insert_with`.

Conséquence — un **vecteur d'inflation d'état à coût quasi nul**. Avec un frais
forfaitaire faible, un attaquant peut créer des millions de comptes-poussière
(solde 1 atom) et gonfler l'état de façon permanente. La simulation de stockage
le confirme : en régime saturé, **~33 des 48 GB à 30 ans sont de l'état**, et
cette part est directement pilotée par le nombre de comptes, pas par le trafic
utile. C'est le **seul** terme non borné du modèle de stockage de VinX.

Le fait que l'état soit plat (monnaie pure, pas de storage de contrats) rend le
problème *tractable* — mais ne l'élimine pas.

## Décision proposée

Introduire un **dépôt existentiel** : un solde plancher `EXISTENTIAL_DEPOSIT_ATOMS`
en dessous duquel un compte ne peut **pas exister** avec un solde non nul.

Règle unique, appliquée à tout chemin qui écrit un solde utilisateur :

> Après application, un compte a **soit** `balance == 0` (et `staked == 0`) —
> auquel cas il est **supprimé** de la map — **soit** `balance >= EXISTENTIAL_DEPOSIT_ATOMS`.
> Aucun état intermédiaire (solde ∈ `]0, ED[`) n'est représentable.

Deux effets symétriques :

1. **À la création / au crédit** : un transfert qui laisserait le destinataire
   avec `0 < balance < ED` est **rejeté** (`CoreError::BelowExistentialDeposit`).
   On ne matérialise plus de compte-poussière.
2. **Au débit / à la suppression** : un compte dont le solde tombe à `0` (et sans
   stake ni unbond en cours) est **retiré** de l'état — il cesse d'occuper 60
   octets. C'est le **reap** (moissonnage), la contrepartie qui *rend* de l'espace.

### Choix de la valeur `EXISTENTIAL_DEPOSIT_ATOMS`

Contrainte de conception : ED doit rendre le remplissage d'état **économiquement
absurde** sans exclure les micro-usages légitimes d'une monnaie de paiement.

- `DECIMAL_FACTOR` = 1e18 atoms = 1 VINX.
- Proposition : **`ED = DECIMAL_FACTOR / 1_000` = 0,001 VINX** (1e15 atoms).

Rationnel : le coût pour immobiliser 1 million de comptes-poussière passe de
~0 à **1 000 VINX bloqués** (récupérables, mais immobilisés et donc à coût
d'opportunité réel), tout en laissant un solde plancher négligeable pour un
utilisateur (0,001 VINX). La valeur exacte est un paramètre à caler ; elle **doit
être une constante gravée** (cf. §Conséquences — pas de levier de gouvernance
silencieux qui casserait le déterminisme).

## Modèle

Invariant renforcé, à ajouter au voisinage de `supply_invariant_holds` :

```
∀ compte c dans accounts :  c.balance == 0  ⇒  c est absent de la map
                            c.balance != 0  ⇒  c.balance >= ED  ∨  c.staked > 0
```

La clause `∨ c.staked > 0` est nécessaire : un validateur peut avoir tout mis en
bond (`balance` faible, `staked` élevé). Le stake étant déjà borné par le bond
minimum (100 k VINX ≫ ED), un compte staké n'est jamais poussière — on l'exempte
donc explicitement du plancher de solde, mais **pas** du reap : il n'est
supprimable que si `balance == 0 && staked == 0 && aucun pending_unbond`.

## Conséquences

**Positif**

- Ferme le vecteur d'inflation d'état → la ligne « saturé » de la simulation se
  rapproche de la ligne « actif » ; le stockage à 30 ans reste borné en pratique.
- Le reap **rend** de l'espace (les comptes vidés disparaissent), ce qui est
  qualitativement nouveau : l'état peut *décroître*.
- Cohérent avec l'esprit du projet (nœud complet sur petit matériel, durablement).

**Coûts / pièges**

- **Consensus-critique.** ED entre dans la fonction de transition d'état : deux
  nœuds avec des ED différents divergent. Donc **constante gravée**, testée par un
  test-tripwire (façon ADR 0021), **jamais** gouvernable en silence.
- **Poussière irrécupérable ?** Non : un solde `< ED` ne peut jamais *exister*, donc
  il n'y a pas de résidu bloqué à la Polkadot. Mais un transfert de type « balayage »
  (envoyer tout sauf de quoi payer le frais) doit laisser soit `0`, soit `>= ED` —
  le wallet doit gérer ce cas (voir Notes).
- **UX wallet.** Envoyer un montant qui laisserait l'expéditeur ou le destinataire
  sous ED doit être refusé côté client avec un message clair, pas seulement rejeté
  par le nœud.
- **Réconciliation de l'invariant de masse.** Le dust d'un compte reapé est
  **détruit** (ADR 0040) : `circulating_supply −= dust` et `destroyed_atoms += dust`.
  L'invariant `circulating + destroyed = emitted` (ADR 0040) tient. À vérifier
  explicitement dans les tests.
- **Genesis / La Fonderie.** La Fonderie est supprimée (ADR 0040) — non concernée.
  Le validateur genesis est staké — exempté du plancher.

## Alternatives écartées

- **State rent (loyer d'état)** : faire payer un loyer périodique par compte.
  Puissant mais lourd (il faut un balayage temporel de tout l'état, une notion de
  compte « expiré », une resurrection) — c'est le périmètre de l'ADR 0013, pas de
  celui-ci. ED est le **minimum viable** anti-bloat, complémentaire (pas
  concurrent) du loyer.
- **Frais de création de compte** (surcoût one-shot au premier crédit) : équivalent
  fonctionnel mais asymétrique (ne rend jamais d'espace, pas de reap) et complique
  le calcul de frais. ED est plus propre.
- **Ne rien faire** : rejeté — laisse le seul terme non borné du stockage ouvert à
  l'abus.

## Notes d'implémentation

1. `crates/vinx-core/src/amount.rs` : ajouter
   `pub const EXISTENTIAL_DEPOSIT_ATOMS: u128 = DECIMAL_FACTOR / 1_000;` + un
   test-tripwire l'épinglant (comme `test_emission_schedule_is_constitutional`).
2. `crates/vinx-core/src/error.rs` : ajouter `CoreError::BelowExistentialDeposit`.
3. `crates/vinx-state/src/world_state.rs` :
   - `apply_transfer` : après crédit du destinataire, si
     `0 < receiver.balance < ED` → `Err(BelowExistentialDeposit)` (avant les
     `mark_dirty`, pour ne pas polluer l'état sur un rejet). Idem contrôle sur
     l'expéditeur post-débit (interdire de le laisser en poussière ; l'autoriser
     seulement s'il tombe exactement à `0`).
   - Introduire un helper `reap_if_empty(&mut self, addr)` : si
     `balance == 0 && staked == 0 && pas de pending_unbond` → `accounts.remove` +
     marquer la ligne à supprimer côté persistance (redb).
   - Appeler `reap_if_empty` sur l'expéditeur en fin de `apply_transfer`, et dans
     `credit`/`apply_unstake` là où un solde peut atteindre 0.
   - `credit` : appliquer le même plancher (ne pas matérialiser un crédit sous ED
     sur un compte neuf).
   - Ajouter `existential_invariant_holds()` (debug/tests) : parcourt la map,
     vérifie l'invariant du §Modèle.
4. **Persistance (redb).** Le reap doit **effacer** la ligne du store, pas juste
   la retirer de la map en mémoire — sinon elle ressuscite au reload. Étendre le
   canal `persist_dirty` d'un simple « upsert » vers « upsert | delete » (p.ex. un
   `BTreeSet<Address>` de suppressions à appliquer au flush).
5. **Clients** (`vinx-wallet`, `vinx-desktop-core`, SDK, UI) : refuser en amont
   tout montant laissant l'une ou l'autre partie dans `]0, ED[` ; proposer un mode
   « tout envoyer » qui vide exactement à 0.
6. Tests : compte-poussière rejeté ; balayage à 0 → compte reapé et absent du
   Merkle ; `circulating_supply` inchangé par un reap (ADR 0004) ; reload après
   reap ne ressuscite pas le compte ; validateur à bond exempté du plancher mais
   non reapé tant que `staked > 0`.

## État de l'implémentation

Implémenté conformément au design ci-dessus, avec une précision de sûreté importante :

- `EXISTENTIAL_DEPOSIT_ATOMS = DECIMAL_FACTOR / 1_000` (0,001 VinX) + test-tripwire
  `test_existential_deposit_is_constitutional` (constante gravée, non gouvernable).
- `CoreError::BelowExistentialDeposit`.
- `apply_transfer` valide l'ED **avant toute mutation**, via un calcul de deltas par
  adresse (expéditeur, payeur de frais, destinataire — gère `from == to` et
  `sponsor == to`). **Raison de sûreté :** le producteur applique chaque tx et, sur erreur,
  **saute la tx sans rollback** ; une vérification tardive laisserait des mutations
  partielles dans le bloc → divergence. La vérif est donc strictement en amont.
- `reap_if_empty(addr)` : reap si `balance == 0 && staked == 0 && aucun pending_unbond`
  (sinon les fonds en cours de déliaison seraient brûlés). Appelé sur les trois parties en
  fin de `apply_transfer`. Le reap force un `needs_rebuild` du Merkle et **marque la ligne
  pour suppression** en persistance.
- **Persistance (redb).** `StateWrite.account_deletes` : une adresse *dirty* absente de la
  map ⇒ ligne effacée du store (`serialize_incremental` la classe en delete,
  `write_state` la supprime). Garantit qu'un compte reapé **ne ressuscite pas** au reload.
- `existential_invariant_holds()` (debug/tests) : `∀ c, staked>0 ∨ balance>=ED`.
- `credit` (récompenses, maturation d'unbond, bounty) **inchangé** : ses bénéficiaires sont
  soit des validateurs (`staked > 0`, exemptés), soit des retours de bond `>= MIN_STAKE`
  (1 VinX ≫ ED) — jamais de poussière. Y appliquer un rejet casserait l'invariant de masse
  (on ne peut pas « refuser » une récompense de bloc). Le vecteur de poussière réel (les
  transferts) est fermé côté `apply_transfer`.
- Tests : compte-poussière rejeté (création et balayage laissant l'expéditeur en poussière),
  transfert d'exactement ED accepté, balayage à 0 → reap + `circulating_supply` inchangé,
  compte staké exempté du plancher et non reapé, compte tout-en-déliaison non reapé jusqu'à
  maturation, et non-résurrection après reload (persistance).

**Reste (non-consensus, UX) :** le garde-fou **côté clients** (`vinx-wallet`,
`vinx-desktop-core`, SDK, UI) — refuser en amont un montant laissant une partie dans
`]0, ED[` et proposer un mode « tout envoyer » qui vide exactement à 0. Le nœud applique
déjà la règle (frontière de sécurité) ; le travail client n'améliore que le message d'erreur
et ne touche pas le consensus.

## Portée / ce que cet ADR ne fait PAS

- Ne touche **pas** l'émission, les frais, ni le bond de validateur.
- Ne fait **pas** de state rent ni de resurrection (→ ADR 0013).
- N'est **pas** un gel de compte : le protocole n'interdit rien à l'utilisateur,
  il refuse seulement de matérialiser un solde inférieur au plancher.
