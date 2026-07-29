# ADR 0008 — chain_id par défaut sûr

- **Statut :** Proposé
- **Catégorie :** Robustesse · **Priorité :** 🟠 moyenne
- **Date :** Juillet 2026

## Contexte

Tous les constructeurs de `Transaction` codent en dur `CHAIN_ID_DEVNET`, et le champ
`chain_id` porte `#[serde(default = "default_chain_id")]` qui **retombe aussi sur
DEVNET**. Conséquence : une transaction désérialisée **sans `chain_id` explicite** est
silencieusement liée à DEVNET.

Pour un projet qui met en avant la protection anti-replay (chain_id dans les
`signing_bytes`, façon EIP-155), un défaut silencieux vers un réseau donné est un
**piège** : une tx forgée pour un contexte pourrait être acceptée sur devnet, ou une tx
mal formée passer inaperçue.

## Décision proposée

Supprimer le défaut silencieux. Deux options :

- **(a)** Pas de `serde(default)` sur `chain_id` → le champ est **obligatoire** dans le
  format wire ; une tx sans chain_id est rejetée à la désérialisation.
- **(b)** Un défaut **réservé invalide** (ex. `0`) qui **échoue toujours** la validation
  (`check_replay_and_ttl`), forçant un chain_id explicite.

Recommandation : **(b)** (rétro-compatible côté format, échec explicite côté logique). Les
constructeurs exigent un `chain_id` explicite (le chemin `vinx-desktop-core::build_*` le
fait déjà).

## Conséquences

- **+** Plus de liaison devnet accidentelle ; désérialisation plus stricte ; anti-replay
  crédible.
- **−** Casse les tx sérialisées qui comptaient sur le défaut (aucune en pré-mainnet).

## Alternatives écartées

- **Garder le défaut DEVNET** : rejeté — piège anti-replay incompatible avec un mainnet
  public.

## Notes d'implémentation

Réserver `CHAIN_ID_INVALID = 0` ; faire échouer `check_replay_and_ttl` si `tx.chain_id`
n'est pas un id connu. Auditer les constructeurs pour qu'aucun ne code DEVNET en dur.
