# ADR 0022 — Durcissement P2P / anti-DoS

- **Statut :** Accepté — ✅ tranche 1 implémentée (non-breaking ; réseau uniquement) ;
  peer-scoring de mesh différé (tranche 2)
- **Catégorie :** Réseau P2P · **Priorité :** 🟠 moyenne
- **Date :** Juillet 2026

## Contexte

La couche P2P (gossipsub libp2p) exposait plusieurs surfaces d'abus qu'un pair malveillant
pouvait exploiter à coût quasi nul :

1. **Bombe de décompression (zip-bomb).** `P2pMessage::decode` faisait
   `zstd::decode_all(payload)` **sans borne de sortie**. Quelques kilo-octets compressés de
   zéros décompressent en gigaoctets → **OOM du nœud** avec un seul message. C'était la
   faille la plus grave.
2. **Pas de plafond de taille de message.** Aucun `max_transmit_size` explicite : on
   dépendait du défaut libp2p (64 Kio), qui par ailleurs **empêchait** de gossiper un bloc
   plein (un bloc de 10 000 tx pèse ~2,5 Mo) — incohérence latente.
3. **Sync non borné.** Le serveur de `SyncRequest` calculait `from_height + limit as u64`
   (**panique possible** sur overflow avec un `from_height` proche de `u64::MAX`) et pouvait
   renvoyer un `SyncResponse` de taille arbitraire (jusqu'à toute la chaîne d'un coup).
4. **Réputation minimale.** Un seul cas pénalisé (message indéchiffrable, dock de −1, ban à
   −5). **Aucune limite de débit** : un pair pouvait inonder de `NewTransaction` valides sans
   jamais être sanctionné.

## Décision — tranche 1 (implémentée)

Une couche de défense en profondeur, additive (aucun changement de format ni de consensus).

### 1. Garde anti-bombe de décompression

`decode` borne la **sortie décompressée** à `MAX_DECODED_BYTES = 16 Mio` via un décodeur
zstd en flux tronqué à `max + 1` octets : atteindre ce seuil ⇒ rejet. La mémoire allouée par
message est bornée à `MAX + 1`, **quelle que soit** la taille du cadre compressé. Le chemin
`FLAG_RAW` rejette aussi tout payload > `MAX_DECODED_BYTES`.

### 2. Plafond de taille de message

`max_transmit_size(MAX_DECODED_BYTES)` sur la config gossipsub : borne la taille **on-wire**
(compressée) et, au passage, **lève** le plafond 64 Kio pour que blocs pleins et lots de sync
passent. Le compressé est borné par cette valeur ; le décompressé, par le garde §1 — les deux
axes sont fermés.

### 3. Bornes de sync

Côté serveur `SyncRequest` :
- `limit` **clampé** à `MAX_SYNC_RESPONSE_BLOCKS = 512` ;
- arithmétique **saturante** (`saturating_add`) — plus de panique sur overflow ;
- le lot est **tronqué à un budget d'octets** (`SYNC_RESPONSE_BUDGET_BYTES = 8 Mio`) via
  l'aide pure `sync_batch_len`, qui prend au moins un bloc (progression garantie même si un
  bloc dépasse seul le budget). Un `SyncResponse` ne peut donc jamais dépasser
  `MAX_DECODED_BYTES`.

### 4. Rate-limiting par pair + réputation (`p2p::guard`)

Nouveau module `PeerGuard` combinant :
- un **token bucket par pair** (`PEER_MSG_BURST = 200`, `PEER_MSG_RATE_PER_SEC = 50`) : tout
  message inbound consomme un jeton ; en dépassement, le message est **jeté** et le pair
  pénalisé (`RATE_FLOOD_PENALTY`) — un flot soutenu finit par franchir le seuil de ban ;
- un **registre de réputation** : dock sur message indéchiffrable/surdimensionné/bombe
  (`BAD_MESSAGE_PENALTY`) et sur inondation ; à `≤ BAN_THRESHOLD = -5`, la boucle blackliste
  le pair au niveau gossipsub.

Le temps est injecté (`now: Instant`) → logique de débit **pure et testée** sans `sleep`.
L'état par pair est purgé à la déconnexion (`forget`) pour borner la mémoire face au churn.

## Conséquences

**Positif**
- Ferme le vecteur OOM le plus grave (zip-bomb) et borne la mémoire par message.
- Un pair inondeur est désormais throttlé puis banni automatiquement.
- Le sync ne peut plus faire paniquer ni saturer le nœud ; réponses bornées et progressives.
- Débloque le gossip de blocs pleins (plafond relevé), corrigeant une incohérence latente.

**Coûts / limites**
- Les seuils (`16 Mio`, `50 msg/s`, burst `200`, budget `8 Mio`) sont des **constantes
  d'exploitation**, pas des paramètres de consensus — ajustables sans fork, mais à caler avec
  un vrai trafic (banc multi-nœuds).
- Le rate-limit est **par pair** (PeerId), pas par IP ni par sous-réseau : un adversaire
  multi-identités contourne partiellement — d'où la tranche 2.

## Tranche 2 (différée, encadrée)

- **Peer-scoring de mesh gossipsub** natif (paramètres P₁–P₇ : temps en mesh, first-message
  deliveries, invalid-message rate, IP-colocation) — le mécanisme de scoring intégré de
  libp2p, plus riche que notre réputation maison, avec **décroissance temporelle** du score.
- **Pénalité sur contenu sémantiquement invalide** (bloc mal signé, invariant de masse violé,
  co-signature invalide) : nécessite de faire remonter un *verdict* de `dispatch_message`
  jusqu'à la boucle — plomberie volontairement reportée pour garder ce diff net.
- **Fast-sync par checkpoints / weak subjectivity** (recouvre l'ADR 0014) : robustesse du
  protocole de sync au-delà du simple lot borné (back-pressure sur plusieurs pairs,
  vérification de finalité par checkpoint).
- **Rate-limit par IP/sous-réseau** en complément du par-pair, contre les identités Sybil au
  niveau transport.

## Alternatives écartées

- **Désactiver la compression** pour éliminer la bombe : rejeté — la compression est utile
  (blocs/sync volumineux) et le garde borné §1 neutralise la bombe sans la perdre.
- **Ne rien borner et compter sur libp2p** : rejeté — le défaut 64 Kio cassait les blocs
  pleins, et rien ne bornait le décompressé ni le débit applicatif.

## Notes d'implémentation

- `crates/vinx-node/src/p2p/messages.rs` : `MAX_DECODED_BYTES`, `decompress_bounded`,
  garde `FLAG_RAW`, `MAX_SYNC_RESPONSE_BLOCKS`, `SYNC_RESPONSE_BUDGET_BYTES`,
  `sync_batch_len` (pure). Tests : bombe rejetée, raw surdimensionné rejeté, message légitime
  toujours décodé, batching.
- `crates/vinx-node/src/p2p/guard.rs` : `TokenBucket`, `PeerGuard`, `Admit`, constantes de
  seuils. Tests : burst/throttle, refill/plafond, flood→ban, pair honnête jamais pénalisé.
- `crates/vinx-node/src/p2p/mod.rs` : `max_transmit_size`, rate-limit avant tout traitement,
  pénalité de décodage via `PeerGuard`, `forget` à la déconnexion, bornes du serveur de sync.
