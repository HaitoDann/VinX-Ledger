# ADR 0018 — Observabilité & SLO

- **Statut :** Proposé — 🟢 future
- **Date :** Août 2026
- **Portée :** Exploitation — métriques, alertes et objectifs de service pour le réseau VinX.
- **Décideur :** VinX Labs.

---

## 1. Problème

VinX expose déjà des métriques Prometheus et un dashboard Grafana (via `docker-compose.yml`),
mais aucun **SLO (Service Level Objective)** formalisé ni aucune **alerte** n'est défini.
Résultat : on ne sait pas quand la chaîne dégrade avant que ce soit visible.

## 2. Métriques existantes (à conserver et étendre)

- `vinx_block_height` — hauteur courante.
- `vinx_finalized_height` — hauteur finalisée.
- `vinx_mempool_size` — transactions en attente.
- `vinx_peer_count` — pairs connectés.
- `vinx_consensus_round_ms` — latence de production de bloc.

## 3. SLO cibles (à valider)

| Métrique | SLO | Alerte |
|---|---|---|
| Latence de bloc | ≤ 15 s (p95) | > 30 s pendant 2 min |
| Finalité lag | `height - finalized_height` ≤ 3 | > 10 blocs pendant 1 min |
| Mempool backlog | ≤ 50 000 tx | > 80 000 tx |
| Peers connectés | ≥ 3 | < 2 pendant 30 s |
| Uptime validateur | ≥ 99 % sur 24 h | (déclenche le jailing — ADR 0027) |

## 4. Direction

- Formaliser les SLO ci-dessus (valeurs à affiner en testnet).
- Ajouter des métriques manquantes : `vinx_epoch_emission_atoms`, `vinx_slash_events_total`,
  `vinx_reorg_total`, `vinx_p2p_ban_total`.
- Écrire les règles d'alerte Prometheus (`alerting_rules.yml`).
- Documenter le runbook pour chaque alerte (dans `GUIDE.md` ou un doc dédié).

## 5. Critères de validation (à écrire avant Accepté)

- [ ] Toutes les métriques SLO sont exposées sur `/metrics` (Prometheus scrape).
- [ ] Un dashboard Grafana avec les SLO est inclus dans `monitoring/`.
- [ ] Les alertes tirent sur un canal défini (PagerDuty / Slack / email).
- [ ] Les SLO sont vérifiés en testnet sur 72 h de run continu.

## 5. Dépendances

- Phase 2 (banc multi-nœuds requis pour calibrer les valeurs SLO).
