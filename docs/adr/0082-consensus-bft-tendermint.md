# ADR 0082 — Consensus BFT par étapes (Tendermint)

- **Statut :** Accepté ✅ — implémenté
- **Date :** Septembre 2026
- **Portée :** Protocole — consensus, sélection du proposeur, ensemble de validateurs
- **Décideur :** VinX Labs (mainteneur)
- **Remplace / modifie :** ADR 0070 (authentification proposeur), 0071 (verrou de vote),
  les ADR de comité échantillonné et de VRF (retirés), le fork-choice et les compact blocks
- **Crates :** `vinx-core`, `vinx-crypto`, `vinx-state`, `vinx-node`, `vinx-desktop-core`

---

## 1. Contexte

L'ancien consensus mêlait production de bloc par un leader, finalité par co-signatures
après coup, fork-choice et réorganisations. Il en résultait deux notions de « tête »
(tip et finalisé) et plusieurs chemins d'exécution. Pour un rail de paiement, un paiement
doit être **définitif dès qu'il est inclus**. Il ne doit jamais être réorganisé.

## 2. Décisions

### C1 — Consensus BFT par étapes (Tendermint)

Il se déroule en trois étapes : proposition, prevote et precommit, avec verrou (*lock*) et
valeur valide (Buchman, Kwon, Milosevic, 2018). Un bloc est commité par des precommits
représentant **strictement plus de 2/3** de la puissance de vote (`⌊2W/3⌋ + 1`). Il n'y a
plus ni fork-choice ni réorganisation : **tip = finalisé**.

- Le moteur (`vinx-node/src/bft.rs`) est pur et déterministe. Il ne fait aucune E/S ; il
  interagit par un trait `Host` qui fournit `build_block`, `validate_block` et `may_sign`.
- Le `CommitCert` est un agrégat BLS accompagné d'un bitmap des signataires. Chaque bloc
  embarque le certificat du bloc précédent (`last_commit`) et s'y engage par
  `last_commit_hash`. Les récompenses des co-signataires en découlent de façon déterministe.
- Le garde anti-double-signature est persistant, par (hauteur, round, type). Après un
  redémarrage, le lock est restauré depuis le dernier precommit non nul.
- Les timeouts croissent avec le round : proposition 3 s + 1 s/round, votes 1 s + 0,5 s/round.
  Des timeouts de repli et une rediffusion toutes les 5 s compensent le gossip best-effort.
- Une double signature produit une preuve `VoteEquivocation`, qui est slashable.
- Le proposeur des rounds manqués est pénalisé (jailing fondé sur les rounds).

**Conséquence importante :** à n=3, les 3 validateurs sont nécessaires, car 2 sur 3 ne
dépassent pas strictement 2/3. La tolérance à une panne commence à n=4. En cas de perte du
quorum, la chaîne **s'arrête** plutôt que de diverger : on choisit la sûreté avant la vivacité.

### C2 — Proposeur par rotation pondérée par le stake

Les priorités de proposeur suivent la méthode Tendermint ; les validateurs jailed sont
sautés. L'ECVRF est retiré.

### C3 — Suppression du comité échantillonné

Tous les validateurs actifs votent.

### C4 — Votes pondérés par le stake, plafonnés à 10 %

La puissance de vote est calculée par un algorithme de *water-filling* au plafond
`max(10 %, 1/n)`. Aucun validateur ne peut dépasser ce plafond.

### C5 — Au plus 100 validateurs actifs, sélection automatique

Les 100 validateurs les mieux bondés sont retenus. Il n'y a plus de gouvernance de la taille
du set.

**Réponse à la question du ralentissement :** chaque vote est une signature BLS agrégeable,
et le certificat final est un seul agrégat accompagné d'un bitmap. Avec 100 validateurs, un
round représente environ 200 messages par nœud, soit quelques dizaines de ko : c'est
négligeable face à 12 s de temps de bloc. Le plafond de 100 borne ce coût. Le vrai coût est
la latence réseau entre les étapes (≈ 3 allers-retours), et cette latence ne dépend pas du
nombre de validateurs.

### C6 — Temps de bloc : paramètre de protocole de genèse, 12 s au départ

Le champ `block_time_secs` de la genèse est borné entre 1 et 60 s et engagé dans l'état. On
pourra l'abaisser progressivement.

### C7 — Tolérance de dérive d'horloge : 15 s

Un bloc est refusé si son timestamp dépasse `now + 15 s`.

### Reporté à l'étape 4 (staking)

- unbonding de 14 à 21 jours ;
- slashing corrélé ;
- séparation des clés de retrait et de validation.

## 3. Conséquences

- Il n'existe plus qu'un **chemin d'exécution unique** (`execution.rs`), utilisé pour la
  production, la validation, le gossip et la synchronisation.
- Le `base_fee` est déterministe : il dépend du remplissage du bloc précédent et non du
  mempool local.
- La clé BLS du validateur de genèse est obligatoire.
- `STORAGE_VERSION` passe à 23 ; une nouvelle genèse est obligatoire.
- Les modules supprimés sont `consensus.rs`, `producer.rs`, `reorg.rs` et `vrf.rs`.

## 4. Vérification

- **Moteur :** 14 simulations, dont un proposeur équivoque sur 80 graines et une attaque
  scriptée contre le lock. Un test de mutation confirme que retirer le lock fait échouer le
  test.
- **Pile complète** (`tests/consensus_sim.rs`) : plusieurs nœuds combinant la vraie exécution
  et le vrai moteur, reliés par un réseau déterministe à horloge virtuelle. Scénarios
  couverts :
  - n=4 commite et tous les nœuds ont le même état ;
  - une panne est tolérée et le proposeur absent est pénalisé ;
  - deux pannes provoquent l'arrêt de la chaîne sans aucun commit ;
  - les blocs suivants sont refusés :
    - certificat forgé par des tiers ;
    - certificat transplanté sur un autre bloc ;
    - mauvais proposeur ;
    - `last_commit` retiré ;
    - timestamp dans le futur.
- **Non testé à ce jour :**
  - un vrai réseau multi-processus libp2p ;
  - le script `scripts/bench-n3.sh`, qui repose encore sur l'ancien modèle et est à
    réécrire pour n=4.
