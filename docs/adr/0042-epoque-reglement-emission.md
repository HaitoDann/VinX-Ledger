# ADR 0042 — L'époque de règlement de l'émission

- **Statut :** Proposé (design à décider)
- **Catégorie :** Tokenomics · Consensus · **Priorité :** 🔴 haute
- **Date :** Août 2026

> **Note (août 2026) :** cet ADR raisonne sur une contrainte `EPOCH_SECS ≥ HEARTBEAT_INTERVAL_SECS`. Le heartbeat a depuis été aboli (ADR 0045 — cadence fixe 12 s). La borne `EPOCH_SECS ≥ 12 s` reste vraie par construction (le tick est le bloc). Les sections §2.3 et §2.4 restent valides en remplaçant « heartbeat » par « tick de bloc ».
- **Lié :** **définit la « fenêtre / époque »** que supposent — sans la définir — l'émission
  élastique (ADR 0040, `E = r·F` *par époque*), la répartition inter-subnet par le melt
  (ADR 0041, *« sur la fenêtre »*) et les reward pools de subnet (ADR 0039). S'appuie sur
  l'horloge MTP (ADR 0005), le heartbeat (ADR 0038), l'invariant de masse (ADR 0004), la
  finalité (ADR 0002) ; cohérent avec le fork-choice (ADR 0031) et le modèle à deux couches
  frais/émission (ADR 0040).

---

## 1. Contexte

Trois ADR de tokenomics reposent sur une notion d'**époque** (ou *fenêtre de répartition*)
qu'**aucun ne définit** :

- **ADR 0040** émet `E = r·F` **par unité de temps** et parle d'« émission par époque ».
- **ADR 0041** répartit `E` entre subnets **au prorata du melt sur la fenêtre**.
- **ADR 0039** **abonde les reward pools** des subnets à partir de cette émission répartie.

Il manque donc la **colonne temporelle** commune : *qu'est-ce qu'une époque, quand se
clôt-elle, qui exécute le règlement, et comment le total accumulé est distribué ?*

Et cette colonne se heurte de plein fouet à la **production de blocs à la demande** (ADR 0038 :
au repos, aucun bloc sauf heartbeat) : **la hauteur de bloc et le temps réel sont découplés.**
Une époque comptée en **nombre de blocs** se bloquerait au repos (le melt et l'émission
s'accumulent, mais personne n'est réglé faute de blocs) et défilerait trop vite sous charge.

### Le piège à éviter : payer par transactions

L'intuition naïve — « émettre des transactions pour payer les validateurs et les mineurs » —
est à rejeter : frais sur les récompenses, mempool inondé, non-déterminisme, et surtout ça ne
passe pas à l'échelle. Comme Ethereum (couche consensus), Cosmos (module distribution) et
Bittensor (émission au *tempo*), **une récompense protocolaire est une transition d'état
déterministe, pas une transaction.**

## 2. Décision

> **Une époque est une fenêtre de temps protocole (MTP) de durée `EPOCH_SECS`. À la clôture
> d'une époque, un règlement protocolaire déterministe forge l'émission accumulée sur la
> fenêtre (ADR 0040) et l'abonde dans les reward pools des subnets au prorata du melt (ADR
> 0041/0039). Le règlement est exécuté *paresseusement par le premier bloc dont le MTP
> franchit la frontière d'époque* — jamais par une transaction.**

### 2.1 Horloge d'époque : le temps (MTP), jamais la hauteur

L'époque `k` couvre `[epoch_start + k·EPOCH_SECS, epoch_start + (k+1)·EPOCH_SECS)` en **temps
MTP** (ADR 0005) — la même horloge robuste que l'émission utilise déjà (`curve(now) − emitted`).
Conséquence directe : le block-on-demand **n'a aucun effet** sur *combien* est émis ou réparti,
puisque tout est intégré sur le temps réel, exactement comme l'émission actuelle.

### 2.2 Règlement paresseux au premier bloc qui franchit la frontière

On n'a **pas besoin d'un bloc pile à la frontière**. À chaque `settle_block`, si le MTP du bloc
dépasse `next_epoch_boundary`, on exécute le règlement de l'époque (ou des époques — voir 2.4)
échue(s) *dans ce bloc*, puis on avance le pointeur d'époque. C'est **le même motif de
rattrapage temporel** que `emit_work_reward` (qui forge `curve(t) − emitted` d'un coup) — donc
il compose naturellement avec l'existant.

### 2.3 Le heartbeat borne la latence de paiement au repos

Le heartbeat (ADR 0038, ≥ 1 bloc / 10 min) **garantit qu'aucune frontière d'époque ne reste
non réglée plus de `HEARTBEAT_INTERVAL_SECS`**, même réseau au repos. Les mineurs de subnet sont
donc réglés à ≤ 10 min de la clôture d'époque, sans qu'aucune transaction ne soit nécessaire.
→ *C'est une troisième raison d'être du heartbeat*, en plus du lissage d'émission et de la
preuve de liveness.

### 2.4 Rattrapage d'époques (repos > 1 époque)

Si le réseau est resté inactif plus d'une époque, le bloc qui reprend la main peut franchir
**plusieurs** frontières. Le règlement **boucle** sur les époques échues (borné). Pour garder
ce rattrapage petit et le coût par bloc prévisible, on impose **`EPOCH_SECS ≥
HEARTBEAT_INTERVAL_SECS`** : au repos, un heartbeat tombe dans chaque époque, donc au plus une
époque est réglée par bloc en régime normal.

### 2.5 Le règlement est O(nombre de subnets), pas O(nombre de mineurs)

Point clé de mise à l'échelle, offert par l'ADR 0039 : **les mineurs *tirent* leur dû** d'un
`reward_root` Merkle par preuve d'inclusion — ils ne sont **pas** crédités un par un par la L1.
Le règlement d'époque ne fait donc qu'**abonder les reward pools *par subnet*** (ADR 0039 §2),
soit un travail **borné par le nombre de subnets** (petit, avec porte de bond ADR 0041), **pas**
par le nombre de participants. Le bloc-frontière reste léger. La répartition *intra*-subnet
(quel mineur touche quoi) demeure hors-chaîne, adossée au bond de l'opérateur (ADR 0023).

### 2.6 Déterminisme (fork-safe)

Toutes les entrées du calcul sont **on-chain à la hauteur du bloc-frontière** : niveau de la
Fonderie `F` (pour `E = r·F`), melt cumulé par subnet sur la fenêtre, bond de chaque subnet
(porte + plafond, ADR 0041). Chaque nœud rejoue le bloc-frontière et **recalcule la même
répartition** → même `state_root`. Le règlement passe donc l'invariant de masse (ADR 0004 :
Fonderie −E, pools +E) et la vérification `state_root` du chemin d'application — y compris au
**rejeu de réorg** (ADR 0031), puisqu'il est purement fonction de l'état.

## 3. Ce que l'époque règle — et ne règle pas

| Flux | Horloge | Mécanisme |
|---|---|---|
| **Frais de transaction** → producteur | **immédiat**, chaque bloc | inchangé (ADR 0040 : les validateurs vivent des frais) — **aucune époque nécessaire** |
| **Émission** → reward pools de subnet | **époque** (MTP) | règlement paresseux au bloc-frontière (cet ADR) |
| **Émission** → mineurs de subnet | **pull** | preuve Merkle sur `reward_root` (ADR 0039), hors époque L1 |

> **Réconciliation avec la question « payer les validateurs vs les mineurs ».** Sous le modèle à
> deux couches de l'ADR 0040, **les validateurs sont payés par les frais** (immédiat, par bloc)
> et **n'ont pas besoin d'époque**. L'époque sert **l'émission dirigée vers les subnets**. Si à
> l'inverse une part d'émission devait revenir aux validateurs (cf. ADR 0028, *partage au
> quorum*), elle se brancherait sur **le même règlement d'époque** (part validateurs calculée par
> `stake × fiabilité` via le module `reliability` existant, distribuée au bloc-frontière). Le
> **mécanisme d'époque est identique** dans les deux cas ; seul le partage change. → **décision
> ouverte** à trancher avec 0028/0040, sans impact sur cette colonne temporelle.

## 4. Paramètres

| Paramètre | Valeur proposée | Justification |
|---|---|---|
| `EPOCH_SECS` | **~1 h (3600 s)** | assez long pour amortir le règlement et lisser la mesure de melt ; assez court pour ne pas faire attendre. Repère : le *tempo* Bittensor = 360 blocs × 12 s = **72 min**. |
| Contrainte | `EPOCH_SECS ≥ HEARTBEAT_INTERVAL_SECS` (600 s) | borne le rattrapage à ~1 époque/bloc au repos (§2.4). |
| Horloge | **MTP** (ADR 0005) | robuste au block-on-demand et à la manipulation de timestamp. |

## 5. Conséquences

- **Positif :** définit la fenêtre manquante de 0039/0040/0041 ; robuste au block-on-demand ;
  paiements sans transactions ; déterministe et fork-safe ; règlement léger (O(subnets)) ;
  neutre pour le *total* émis (piloté par le temps, ADR 0040/0021).
- **À implémenter (dépend de 0039/0040/0041, non encore codés) :** un accumulateur de melt par
  subnet sur la fenêtre, le pointeur d'époque dans `WorldState`, et le hook de règlement dans
  `settle_block` (juste après le rattrapage d'émission). Pré-requis : 0040 (émission élastique)
  et 0039 (reward pools) doivent être arbitrés d'abord — cet ADR ne fait que **poser l'horloge**.
- **Négatif / vigilance :** latence de paiement bornée par le heartbeat (≤ 10 min au repos) —
  acceptable pour une distribution d'émission ; le choix `EPOCH_SECS` est un paramètre de
  politique (non constitutionnel — l'immuabilité de 0021/0040 porte sur la *loi* d'émission, pas
  sur la granularité de règlement).

## 6. Alternatives écartées

- **Époque en nombre de blocs** — se bloque au repos et défile trop vite sous charge (§1).
  Rejetée : incompatible avec le block-on-demand.
- **Paiement par transactions protocolaires** — frais, mempool, non-déterminisme, ne scale pas
  (§1). Rejetée au profit d'une transition d'état.
- **Crédit par bloc à chaque bénéficiaire** (modèle actuel étendu) — grumeleux, coûteux en
  écritures d'état, et sans fenêtre de mesure pour le melt (0041). Rejeté au profit du règlement
  d'époque batché.
