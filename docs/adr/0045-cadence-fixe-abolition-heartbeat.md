# ADR 0045 — Cadence fixe 12 s : abolition des blocs à la demande et du heartbeat

- **Statut :** ✅ Accepté et implémenté.
- **Catégorie :** Consensus · Simplification · **Priorité :** 🔴 haute
- **Date :** Août 2026
- **Liens :** étend ADR 0043 (block time fixe 12 s) en supprimant les deux régimes restants
  (skip-empty + heartbeat) ; neutre pour l'émission (ADR 0040/0021 : intégrée sur le temps
  réel, pas sur la hauteur).

---

## 1. Contexte

L'ADR 0043 a fixé le block time à 12 s et supprimé l'accélération dos-à-dos. Il a néanmoins
conservé deux comportements hérités :

1. **Skip-empty** : si le mempool est vide, le producteur ne produit pas de bloc et attend le
   prochain signal de transaction.
2. **Heartbeat** (ADR 0038-heartbeat, `HEARTBEAT_INTERVAL_SECS = 600`) : si aucune transaction
   n'arrive pendant 10 minutes, un bloc vide est produit pour faire avancer l'horloge MTP,
   maturer les unbonds et activer les upgrades.

Ce double régime (on-demand + heartbeat de secours) ajoute de la complexité sans apporter de
valeur réelle dès lors que la cadence est fixe à 12 s. Il génère aussi des comportements
contre-intuitifs : la clôture des époques (ADR 0028), la progression du score de fiabilité
(ADR 0038-v2) et le warmup des nouveaux validateurs deviennent dépendants du niveau d'activité
transactionnelle, ce qui est indésirable.

## 2. Décision

**Supprimer les blocs à la demande et le heartbeat.** Le producteur produit un bloc toutes les
`block_time_secs` (12 s) de manière **inconditionnelle**, quel que soit le contenu du mempool.

Les blocs vides (zéro transaction) sont donc normaux et attendus en période de faible activité.

### Ce qui change

| Comportement | Avant | Après |
|---|---|---|
| Mempool vide au réveil | Skip — repart attendre | **Produit un bloc vide** |
| Attente d'un signal tx | `tx_ready.notified()` | **Supprimé** — tick fixe |
| Heartbeat toutes les 10 min | Oui | **Supprimé** — le tick fixe couvre ce rôle |
| Constante `HEARTBEAT_INTERVAL_SECS` | 600 | **Retirée** |

### Ce qui ne change pas

- Le plancher de 12 s entre blocs (déjà garanti par ADR 0043).
- Le mécanisme de slot-skip / backup validator (ADR 0027) : si le leader est absent après
  2 × block_time, un backup prend le relais.
- L'émission calculée sur le temps réel (ADR 0040) : les blocs vides n'émettent pas plus
  ni moins qu'une fenêtre de temps équivalente avec des blocs pleins.

## 3. Justification

**Simplification radicale de la boucle de production.** Au lieu d'une machine à états avec
trois régimes (stalled / have_work / heartbeat), la boucle devient :

```
loop {
    sleep(block_time);
    self.tick().await;
}
```

**Comportement des époques prévisible.** Avec 12 s fixes, chaque époque de 3 600 s contient
**exactement 300 blocs**, quel que soit le niveau d'activité. Le warmup de 3 époques = 900
blocs — un signal de fiabilité robuste et uniforme.

**Overhead négligeable.** Un bloc vide à 12 s = en-tête (~200 octets) + zéro transaction =
~200 o toutes les 12 s = ~1,2 Mo/heure ≈ ~10 Mo/jour. Sur un réseau peu actif, c'est
parfaitement acceptable.

**L'émission reste neutre.** `emit_work_reward` calcule `curve(now) − emitted` — l'intégrale
sur le temps réel. Un bloc vide à 12 s émet exactement ce qu'une fenêtre de 12 s de silence
aurait émis avec un heartbeat à 600 s, réparti sur 50 blocs au lieu d'un. La masse totale
émise sur une période donnée est identique.

## 4. Implémentation

### `crates/vinx-core/src/amount.rs`
- Supprimer `HEARTBEAT_INTERVAL_SECS`.

### `crates/vinx-node/src/node.rs` — `run_block_producer`
Remplacer le contenu de la boucle par :

```rust
pub async fn run_block_producer(self: Arc<Self>) {
    let block_time = std::time::Duration::from_secs(self.config.block_time_secs);
    loop {
        tokio::time::sleep(block_time).await;
        match self.tick().await {
            Ok(block) => { /* log + post-block housekeeping */ }
            Err(e) => tracing::debug!(error = %e, "Block tick skipped"),
        }
    }
}
```

Le backup-validator (`try_backup_production`) reste inchangé — il s'appuie sur
`last_block_instant` qui est mis à jour à chaque bloc produit ou reçu.

### ADR 0043 — mise à jour
Mettre à jour le tableau de l'ADR 0043 pour refléter que le skip-empty et le heartbeat ont
été retirés dans ce commit (ADR 0045 les supersède sur ces deux points).
