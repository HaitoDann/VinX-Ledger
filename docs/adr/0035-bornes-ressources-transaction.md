# ADR 0035 — Bornes de ressources par transaction

- **Statut :** Proposé
- **Catégorie :** Données, état & scaling / Sécurité · **Priorité :** 🟠 moyenne
- **Date :** Juillet 2026
- **Liens :** complète le durcissement P2P (ADR 0022) côté machine d'état ; cohérent avec
  l'anti-bloat (ADR 0026) et les poids de tx (ADR 0009).
- **Partiellement implémenté :** septembre 2026 — `MAX_TX_PAYLOAD_BYTES = 16 KiB`, appliqué
  au niveau consensus (finding VINX-13).

> ⚠️ **Correction (septembre 2026, finding VINX-13).** `payload` était **non borné** alors que
> les frais dérivent de `amount`, pas de la taille : une transaction de valeur nulle portant
> des mégaoctets coûtait le plancher de frais et rien de plus — du gonflement de bloc gratuit,
> qui évince aussi le trafic réel du mempool de chaque pair. La borne est appliquée dans
> `check_replay_and_ttl`, la porte partagée par `apply_transaction` et
> `apply_transaction_trusted` : c'est donc une **règle de consensus**, pas un simple filtre de
> mempool — sans quoi un payload surdimensionné entrait quand même en état via un bloc et les
> nœuds divergeaient sur la validité des blocs. Régression :
> `oversized_payload_is_rejected_on_every_path`.
>
> Le reste de cet ADR (poids de bloc, bornes par type de transaction) demeure **Proposé**.

## Contexte

La taille d'un **message P2P** est bornée à la réception (`MAX_DECODED_BYTES = 16 Mio`, ADR
0022), et le mempool borne le **nombre** de tx (`max_size`, `MAX_PER_ADDRESS`). Mais **rien
dans la machine d'état** ne borne les ressources d'**une transaction individuelle** :

- le champ `payload: Vec<u8>` (utilisé par `AdminAction`, `AnchorState`, `SlashValidator`)
  n'a **aucun plafond de taille** au niveau consensus ;
- une seule tx à payload énorme (jusqu'à la borne P2P de 16 Mio) est **valide** si elle est
  signée et paie le fee forfaitaire — or le fee est **plat**, indépendant de la taille
  (`calculate_fee` ignore le montant *et* la taille).

Conséquence : un attaquant peut fabriquer des tx **lourdes à coût forfaitaire faible**,
gonflant les blocs et l'I/O de tous les nœuds bien au-delà de ce que le fee couvre. Le fee plat
est excellent pour un **paiement** (prix par ressource, pas par valeur — ADR 0009/whitepaper),
mais il ne **price pas la taille** des payloads spécialisés.

## Décision proposée

Introduire des **bornes de ressources dures, consensus-critiques**, vérifiées dans la
transition d'état (donc identiques sur tous les nœuds), en plus des bornes P2P/mempool.

1. **Plafond de taille de payload par type de tx** (`MAX_PAYLOAD_BYTES`, éventuellement
   différencié) : un `AdminAction`/`AnchorState`/`SlashValidator` dont le payload dépasse la
   borne est **rejeté** à l'application. Les payloads légitimes sont petits et bornés par
   nature (`GovernanceAction`, `ModuleOp`, `SlashEvidence` — deux en-têtes + deux signatures) →
   la borne les couvre largement.
2. **Poids de bloc borné par la ressource, pas seulement par le nombre de tx** : aujourd'hui
   `max_block_txs` borne le *compte*. Ajouter une borne de **poids agrégé** (somme des tailles/
   poids des tx d'un bloc ≤ `MAX_BLOCK_WEIGHT`) → un bloc de peu de tx mais très lourdes reste
   borné.
3. **(À débattre) fee proportionnel à la taille pour les payloads** : garder le forfait plat
   pour un `Transfer` nu (paiement pur), mais ajouter une **composante par octet** au-delà d'un
   seuil pour les payloads volumineux → aligne le coût sur la ressource consommée sans casser
   le modèle de paiement. Alternative plus simple : la borne dure (1/2) suffit peut-être, et on
   évite de complexifier le fee.

## Modèle

- Bornes **gravées** (consensus-critiques) — un nœud avec une borne différente divergerait.
  Épinglées par test-tripwire (façon ADR 0021/0026).
- Vérification **avant mutation** (comme l'ED, ADR 0026) : une tx surdimensionnée est rejetée
  sans effet de bord, donc ne consomme pas de nonce (le producteur saute une tx échouée sans
  rollback).
- Cohérence des bornes : `MAX_PAYLOAD_BYTES ≤ MAX_BLOCK_WEIGHT ≤ MAX_DECODED_BYTES` (P2P) — la
  couche interne est toujours plus stricte que la couche transport.

## Conséquences

**Positif**
- Ferme un vecteur de **bloat/DoS à coût forfaitaire** que le fee plat ne price pas.
- Rend le **poids d'un bloc borné** indépendamment du contenu → temps de validation/I/O
  prévisibles (utile pour la mise à l'échelle et la vérif parallèle de l'ADR 0015).
- Défense en profondeur : la machine d'état ne dépend plus de la seule borne transport.

**Coûts / pièges**
- Consensus-critique : les bornes entrent dans la validité d'un bloc → à figer et tester
  rigoureusement (une borne trop basse rejetterait des tx légitimes futures — la calibrer avec
  marge).
- Si des poids agrégés sont suivis, possible **bump de version** (mais ce sont surtout des
  *checks*, pas forcément du nouvel état persistant).
- La composante fee par octet (point 3) touche l'UX de frais → à ne faire que si la borne dure
  ne suffit pas ; ne **pas** casser le forfait plat du paiement nu.

## Alternatives écartées

- **S'appuyer sur la seule borne P2P (0022)** : insuffisant — 16 Mio par tx signée reste énorme
  pour l'état/I/O, et un pair *validateur* honnête peut relayer une telle tx.
- **Fee purement proportionnel à la taille pour tout** : rejeté — casserait le forfait plat qui
  fait la simplicité et l'équité du paiement VinX (ADR 0009). On borne d'abord, on price
  éventuellement seulement le payload volumineux.
- **Ne rien faire** : rejeté — laisse un déséquilibre coût/ressource exploitable.

## Notes d'implémentation

- `vinx-core/amount.rs` : `MAX_PAYLOAD_BYTES`, `MAX_BLOCK_WEIGHT` (+ tripwire constitutionnel).
- `vinx-state` : rejet des tx à payload surdimensionné dans `dispatch_tx`/handlers (avant
  mutation) ; borne de poids agrégé à la construction (producteur) et à la validation (P2P/sync).
- `vinx-node/producer` : `drain` respecte `MAX_BLOCK_WEIGHT` en plus de `max_block_txs`.
- Tests : payload à la borne accepté / au-delà rejeté ; bloc au poids max ; pas d'effet de bord
  sur rejet ; cohérence des trois bornes.
- Dépendance : cohérent avec ADR 0022 (borne transport) et ADR 0015 (poids prévisible pour la
  vérif parallèle).
