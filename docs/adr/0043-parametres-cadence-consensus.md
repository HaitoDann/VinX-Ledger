# ADR 0043 — Paramètres de cadence de consensus : block time fixe et plancher anti-fork

- **Statut :** ✅ Accepté (paramètres appliqués)
- **Catégorie :** Consensus · Robustesse · **Priorité :** 🔴 haute
- **Date :** Août 2026
- **Lié :** supprime l'accélération de la production à la demande (ADR 0038, conservé pour le
  heartbeat) ; réduit la surface du fork-choice (ADR 0031) ; s'appuie sur la propagation compacte
  (ADR 0037) et les bornes de ressources de bloc (ADR 0035) ; neutre pour l'émission (intégrée
  sur le temps, ADR 0021/0040).

---

## 1. Contexte

La cadence de production actuelle (`dynamic_gap`, `node.rs`) **accélère avec la charge** :
l'écart entre blocs rétrécit à mesure que le mempool se remplit, jusqu'à **zéro (blocs
dos-à-dos)** en saturation. Combiné à un `block_time` de **5 s**, cela **maximise la fenêtre de
fork** exactement quand le trafic est le plus dense : deux producteurs (leader + backup, ou
leader lent) peuvent sceller à la même hauteur avant que le réseau n'ait convergé.

On veut **conserver une vitesse correcte tout en réduisant le risque de fork**, en s'alignant
sur les standards éprouvés (Ethereum & Bittensor : 12 s ; Substrate/Polkadot : 6 s).

## 2. Décision

| Paramètre | Avant | Après | Justification |
|---|---|---|---|
| **Block time** | 5 s | **12 s** (défaut) | marge large : propagation (~2 Mo) + validation (~10 000 vérifs de signatures ≈ 0,5–1 s) « 12 s → forks rares. Aligné Ethereum/Bittensor. |
| **Accélération à la demande** | oui (`gap → 0` en saturation) | **retirée — plancher fixe à `block_time`** | tue les blocs dos-à-dos, le générateur #1 de forks. La congestion est absorbée par le **base-fee** (ADR type EIP-1559), pas par des blocs plus rapprochés. |
| **Poids max de bloc** | 10 000 tx | **10 000 tx (≈ 2 Mo)** | ~200 o/tx → le plafond de 10 000 vaut ≈ 2 Mo. Reste la borne effective de taille (voir §4). |
| **Heartbeat** | 10 min | **Aboli (ADR 0045)** | la cadence fixe 12 s rend le heartbeat superflu — chaque slot est un tick régulier. |
| **Blocs vides au repos** | non (skip + heartbeat) | **Aboli (ADR 0045)** : blocs produits à cadence fixe 12 s, vides ou non — pas de skip-empty, pas de heartbeat. |

## 3. Justification

- **Débit :** 10 000 tx / 12 s ≈ **833 TPS** de crête. Le plafond passe d'« illimité » (dos-à-dos)
  à un **plafond ferme et volontaire** — un arbitrage assumé en faveur de la stabilité. 833 TPS
  soutenus est un excellent chiffre pour une L1 de paiement.
- **Congestion :** absorbée par le **base-fee** (quand c'est plein, les frais montent, la demande
  retombe) — pas besoin de « bursté » pour drainer le mempool.
- **Neutralité tokenomics :** l'émission est `curve(t) − emitted` (intégrale sur le **temps
  réel**, ADR 0021/0040) → changer le block time **n'affecte ni le total émis ni son débit**.
  Vérifié dans `emit_work_reward`.
- **Complémentarité fork-choice :** cette décision rend les collisions **rares** ; le fork-choice
  + réorg (ADR 0031) les traite **correctement** quand elles surviennent. Réduction *et* gestion.

## 4. Vigilance

- **Bascule backup plus lente.** Le timeout de slot est `(distance+1)·block_time` (`node.rs`).
  À 12 s, un leader mort → **~24 s** avant qu'un backup produise. Prix assumé de moins de forks ;
  la finalité (ADR 0002) reste sûre pendant ce temps.
- **Plafond 2 Mo — borne effective vs borne dure.** Aujourd'hui la borne est le **compte de tx**
  (10 000) ; avec ~200 o/tx cela vaut ≈ 2 Mo. Un **plafond dur en octets** (2 Mo) appliqué
  *identiquement* à la production **et** à la validation (sinon désaccord entre nœuds →
  consensus-critique) est un **suivi** distinct, à câbler avec les bornes de l'ADR 0035 si la
  variance de taille des tx grandit.
- **Vérif de signatures.** À 10 000 tx, la vérification doit rester une **fraction** du slot
  (batch/parallélisme) pour laisser de la marge au validateur le plus faible.

## 5. Implémentation

Appliqué dans ce commit :
- `NodeConfig::block_time_secs` défaut **5 → 12** (`config.rs`) ; défauts CLI/fichier alignés
  (`main.rs`).
- **`dynamic_gap` supprimée** ; la boucle de production espace les blocs d'un **plancher fixe
  `block_time`** (plus d'accélération dos-à-dos). Le skip-empty + heartbeat au repos sont
  conservés.

Non inclus (suivis) : plafond dur 2 Mo en octets sur les deux chemins ; batch de vérif de
signatures. Voir §4.
