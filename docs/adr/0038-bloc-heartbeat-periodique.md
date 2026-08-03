# ADR 0038 — Bloc heartbeat périodique

- **Statut :** ✅ Implémenté
- **Catégorie :** Économie · Robustesse · **Priorité :** 🟠 moyenne
- **Date :** Août 2026
- **Lié :** émission fair launch (ADR 0021), partage d'émission (ADR 0028), temps réseau
  MTP (ADR 0005), fork-choice (ADR 0031).

## Contexte

La production de blocs est **à la demande** : au repos, aucun bloc. Trois propriétés du
protocole se combinent alors en une incitation perverse :

1. **L'émission est intégrée sur le temps** (`curve(t) − emitted`) : elle s'accumule
   pendant le repos et le **premier bloc après une période calme forge tout l'accrual
   d'un coup, à un seul producteur** — une prime jackpot aléatoire.
2. **Les frais vont à 100 % au producteur** : le leader du prochain slot qui s'envoie
   une transaction à lui-même récupère ses propres frais dans le même bloc. **Forcer un
   bloc est donc gratuit** pour le leader.
3. Le leader du slot est connu (round-robin) et garde la main jusqu'à production ou
   timeout de slot.

Stratégie rationnelle de chaque validateur : dès son tour, spammer une self-tx pour
capturer l'accrual avant qu'un backup ne le double. Si tous le font — et à l'équilibre,
tous le font — la chaîne converge vers une **production continue (~`block_time`) remplie
de transactions bidon** : ~2–3,5 Go/an de blobs, dont ~1,2 Go/an de headers
définitivement imprunables, pour une distribution d'émission qui aurait été identique
sans le spam. Un dilemme du prisonnier.

Problèmes secondaires du repos total : le **MTP** (horloge protocole depuis l'ADR 0005)
peut dater de plusieurs jours ; une chaîne silencieuse est indistinguable d'une chaîne
morte ; les déliaisons et upgrades ne mûrissent qu'à la faveur d'un bloc. (Un heartbeat
*conditionnel* d'1 h existait déjà pour ces deux derniers cas.)

## Décision

**Heartbeat inconditionnel : au moins un bloc toutes les `HEARTBEAT_INTERVAL_SECS =
600` (10 min), même vide.** La cadence à la demande est conservée sous charge — le
heartbeat n'est que le plancher de cadence au repos.

Effet économique : l'accrual sera de toute façon forgé au prochain heartbeat par le
leader round-robin du moment. Forcer un bloc plus tôt ne déplace que ≤ 10 min d'accrual
— l'incitation à fabriquer de fausses transactions disparaît en pratique, et la prime
jackpot au réveil est remplacée par une distribution lissée sur la rotation. L'ADR 0028
(partage au quorum) achèvera de rendre le timing de capture sans intérêt.

**Pas de plafond d'accrual par bloc.** Le total émis est inchangé par cette décision
(l'émission est une intégrale sur le temps) ; le heartbeat n'en change que la
granularité de distribution. Le whitepaper (« une chaîne inactive ne forge rien »)
décrivait une propriété que l'implémentation n'a jamais eue : l'accrual s'accumulait au
repos et était forgé d'un coup au réveil. Cet ADR acte la doctrine réelle : **l'émission
suit le temps réel, le heartbeat la distribue régulièrement**. (Alternative écartée
ci-dessous.)

## Coût en données (mesuré sur la sérialisation réelle)

| Élément | Taille |
|---|---|
| Header | 144 o |
| Par co-signature | 116 o |
| Bloc vide, 1 validateur | ~290 o |
| Bloc vide, 3 validateurs | ~540 o |
| Header seul après `prune()` | ~190 o |

À 10 min : 52 560 blocs/an ≈ **15–30 Mo/an**, retombant vers ~10 Mo/an une fois les
signatures anciennes prunées. Négligeable — d'autant que le stockage v10 écrit chaque
bloc comme une ligne redb individuelle (pas de réécriture de la chaîne).

## Conséquences

- **+** Supprime l'incitation au spam de capture (le bloat de l'équilibre spam était
  ~100× le coût du heartbeat).
- **+** MTP borné (~55 min de retard max au repos) ; déliaisons et upgrades mûrissent à
  l'heure ; signal de vivacité permanent ; les light clients et la supervision voient
  une chaîne qui avance.
- **−** ~15–30 Mo/an de données. Les validateurs forgent l'émission même sans usage —
  c'est la doctrine actée ci-dessus.
- **=** Au réveil simultané des nœuds (leader + backups au même heartbeat), la course
  éventuelle est résolue comme aujourd'hui (premier bloc reçu gagne, `prev_hash` rejette
  le concurrent) ; le banc n≥2 et l'ADR 0031 (fork-choice) couvrent ce cas.

## Alternatives écartées

- **Plafonner l'accrual forgeable par bloc** (le surplus reste en Fonderie) : donnerait
  un vrai « rien au repos », mais étire la courbe d'émission de façon dépendante de
  l'usage — la promesse « halving 8 ans, courbe gravée » (ADR 0021) deviendrait molle.
  Rejeté tant que la courbe immuable est un engagement du protocole.
- **Statu quo + ADR 0028 seul** : le partage au quorum réduit le gain du timing mais ne
  supprime ni la gratuité du forçage ni le bloat de l'équilibre spam.
- **Heartbeat conditionnel élargi** (mempool non vide OU accrual > seuil…) : complexité
  et surface d'oracle interne pour économiser quelques Mo/an. Rejeté.

## Notes d'implémentation

`HEARTBEAT_INTERVAL_SECS` passe de 3 600 à 600 (`vinx-core/src/amount.rs`) et la
condition `has_pending_time_sensitive_ops()` disparaît de `run_block_producer`
(`vinx-node/src/node.rs`) : au repos, le producteur attend `tx_ready` **ou** l'échéance
du heartbeat, et produit un bloc vide dans le second cas. Le chemin sous charge
(pacing adaptatif, anti-spin sur backlog inapplicable) est inchangé. Les backups
n'interviennent que si le leader du heartbeat est réellement en retard (mécanique de
slot-timeout existante).
