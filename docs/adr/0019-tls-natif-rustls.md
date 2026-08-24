# ADR 0019 — TLS natif (rustls)

- **Statut :** Proposé — 🟢 future
- **Date :** Août 2026
- **Portée :** Réseau — HTTPS natif sur le RPC sans dépendre d'un reverse-proxy.
- **Décideur :** VinX Labs.

---

## 1. Problème

Le nœud VinX expose son API RPC en **HTTP plain** (`0.0.0.0:8080` par défaut). Le
chiffrement TLS est actuellement assuré par **Caddy** (voir `Caddyfile` à la racine) en
tant que reverse-proxy. Cela :
- Ajoute un composant externe (Caddy) à opérer, monitorer et mettre à jour.
- Rend le déploiement standalone sans Caddy **non chiffré** (mauvaise ergonomie pour les
  opérateurs qui ne lisent pas les docs).

## 2. Options envisagées

| Option | Description | Compromis |
|---|---|---|
| A — Garder Caddy | Proxy inverse, certificats Let's Encrypt automatiques | Simple, aucun changement code ; dépendance externe |
| B — TLS natif rustls ✅ | Axum + rustls, certificats chargés depuis le `config.toml` | Un composant de moins ; ALPN/HTTP2 possible |
| C — Self-signed cert auto-généré | Le nœud génère son propre certificat au démarrage | Pas de PKI ; warnings navigateur |

## 3. Direction

- Intégrer **rustls** via `axum-server` ou `axum::serve` + `tokio-rustls`.
- Configuration dans `config.toml` : `[rpc.tls] cert_path`, `key_path`.
- Garder Caddy en option (certains opérateurs préfèrent le reverse-proxy pour d'autres raisons).
- Documenter la migration dans `GETTING_STARTED.md`.

## 4. Critères de validation (à écrire avant Accepté)

- [ ] L'API RPC répond en HTTPS avec un certificat valide.
- [ ] La configuration TLS dans `config.toml` est documentée.
- [ ] Le comportement sans TLS (HTTP) reste possible (config `tls = false`).
- [ ] `cargo test --workspace` vert.

## 5. Dépendances

- Phase 2 (priorité opérationnelle augmente avec plus de nœuds).
