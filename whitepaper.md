# VinX Ledger — Livre blanc

**Version :** 7.0
**Date :** Octobre 2026
**Éditeur :** VinX Labs

---

## 1. Vision

VinX Ledger est un **rail de paiement** : une blockchain de couche 1 qui ne fait qu'une chose,
faire circuler de la valeur rapidement, définitivement et de façon vérifiable.

Ce n'est pas une plateforme de contrats intelligents. Il n'y a ni machine virtuelle, ni
stablecoin, ni jetons secondaires. Un seul token, le **VINX**, et une seule fonction, payer.
C'est un choix de conception (ADR 0064), pas une limite technique : un protocole qui fait une
chose peut rester assez petit pour être lu, audité et exécuté par n'importe qui.

Trois promesses en découlent :

- **Définitif tout de suite.** Un paiement inclus dans un bloc est irréversible au bloc
  suivant. Il n'y a pas de confirmations à attendre ni de réorganisation possible.
- **Léger.** Un nœud ne garde que 30 jours d'historique et un état compact vérifiable. Un PC
  ordinaire, un mini-serveur ou un Raspberry Pi suffisent pour valider.
- **Ouvert.** N'importe qui peut devenir validateur en déposant une garantie. Aucune
  autorisation n'est nécessaire, et aucun serveur central n'est requis.

> **Ce qui change en v7.0 :**
> - Le consensus est désormais **BFT de type Tendermint** (ADR 0082) ; il remplace le PoA à
>   comité VRF.
> - Les **récompenses sont proportionnelles à l'enjeu** (preuve d'enjeu, ADR 0086).
> - Garantie minimale de **1 000 VINX** ; déliaison en **21 jours** ; slashing **corrélé**.
> - **État léger** (ADR 0083) et **mémo de paiement** (ADR 0085).
> - Réseau **sans serveur central**, qui fonctionne **derrière les box internet**.
> - **Mises à jour sans redémarrage**, activées par les validateurs (ADR 0086).

---

## 2. Architecture

| | |
|---|---|
| **Implémentation** | Rust, de bout en bout, sans framework blockchain tiers ni machine virtuelle |
| **Consensus** | BFT Tendermint : proposition, prevote, precommit ; finalité immédiate à plus des 2/3 |
| **Cadence** | Un bloc toutes les **12 s**, paramètre de genèse |
| **Capacité** | Jusqu'à **3 000 transactions par bloc** (~250 tx/s) |
| **Précision** | 9 décimales internes, affichage au centime |
| **Adresses** | Bech32m, préfixe `vinx1` ; adresses lisibles `nom@domaine` résolues hors chaîne |
| **Cryptographie** | Ed25519 (transactions), BLS12-381 (votes agrégés), BLAKE3 (hachage) |
| **État** | Arbre de Merkle creux (SMT) : preuves de solde, reçus vérifiables |
| **Réseau** | libp2p : gossipsub, Kademlia, UPnP, AutoNAT, relais, perçage de NAT (DCUtR) |
| **Référence de temps** | Horodatage des blocs (secondes réelles), jamais la hauteur |

### 2.1 Types de transactions

| Type | Usage |
|---|---|
| `Transfer` | Paiement en VINX, avec un **mémo** facultatif de 32 octets signé (ADR 0085) |
| `Stake` / `Unstake` | Déposer ou retirer la garantie de validateur |
| `RegisterBlsKey` | Enregistrer la clé de vote d'un validateur (avec preuve de possession) |
| `Unjail` | Réhabiliter un validateur suspendu pour absence |
| `SlashValidator` | Soumettre la preuve d'un double vote |
| `SetMemoRequired` | Exiger un mémo sur les paiements reçus (comptes de dépôt, commerçants) |
| `AnnounceUpgrade` / `AdminAction` | Gouvernance de lancement (§8) |

Un nœud VinX n'exécute jamais de code tiers.

### 2.2 État léger (ADR 0083)

- L'état (soldes, garanties, set de validateurs) est permanent et compact. Sa racine de
  Merkle figure dans chaque bloc.
- Les blocs et leurs transactions sont **élagués après 30 jours**. Un nœud qui rejoint le
  réseau démarre depuis un instantané de l'état, signé par le quorum.
- Les **reçus de paiement** (la transaction et sa preuve d'inclusion) restent vérifiables hors
  ligne après l'élagage. C'est le payeur et le bénéficiaire qui les conservent, pas le réseau.
- Un mode `--archive` conserve tout l'historique pour les explorateurs et les auditeurs.

---

## 3. Consensus et validateurs

### 3.1 BFT Tendermint (ADR 0082)

À chaque hauteur, un proposeur choisi selon le poids de vote propose un bloc. Les validateurs
votent en deux temps (prevote, puis precommit). Le bloc est **définitif** dès que plus des 2/3
de la puissance de vote l'ont precommité : il ne peut plus être remis en cause. Les signatures
BLS sont agrégées dans un certificat joint au bloc suivant.

- **Puissance de vote** : la garantie déposée, plafonnée à **10 % par validateur** pour limiter
  la centralisation.
- **Set actif** : jusqu'à **100 validateurs**, classés par présence.
- **Sûreté avant disponibilité** : si plus d'un tiers de la puissance disparaît en même temps,
  la chaîne s'arrête plutôt que de risquer un fork. Elle reprend quand les validateurs
  reviennent.

### 3.2 Entrée libre, set stable (ADR 0086)

- **Entrée** : déposer au moins **1 000 VINX** avec sa clé de vote. Après un **échauffement**
  de 3 époques (environ 3 h), le validateur entre dans le set.
- **Rythme** : au plus **10 % de nouveaux validateurs par époque** (au minimum 1) et 2 sorties
  volontaires par époque. Le set ne peut pas être bouleversé d'un coup.
- **Absences** : un validateur qui manque 3 de ses tours de proposition est **suspendu** et
  **sort du vote immédiatement**. Des absences étalées dans le temps ne s'additionnent donc
  jamais jusqu'au tiers fatal. Revenu en ligne, son nœud le **réhabilite tout seul**.
- **Clés séparées** (ADR 0084) : le nœud ne détient qu'une clé d'opérateur et une clé de vote.
  La clé qui détient les fonds reste hors du nœud.

### 3.3 Sanctions (ADR 0084)

- **Absence** : suspension, sans perte d'argent. Le validateur ne touche plus de récompenses
  tant qu'il est absent.
- **Double vote (équivocation)**, prouvé cryptographiquement : pénalité de **5 % + 3 × la part
  de puissance sanctionnée dans la même fenêtre**, plafonnée à 100 %. Une erreur isolée (un
  nœud lancé deux fois) coûte quelques pour cent ; une attaque coordonnée d'un tiers du réseau
  coûte tout. 10 % de la pénalité revient au rapporteur, et rien n'est brûlé.
- **Déliaison** : une garantie retirée est rendue après **21 jours**, et reste sanctionnable
  pendant ce délai.

---

## 4. Économie

### 4.1 Offre fixe, lancement équitable

L'offre est plafonnée à **1 000 000 000 VINX**, définitivement. Sur le mainnet, **rien n'est
pré-alloué** : à la genèse, zéro VINX existe. Tous les VINX sont créés au fil du temps et
versés à ceux qui font tourner le réseau, fondateur compris.

Les réseaux de test pré-financent un faucet. Ce pré-financement n'entre pas dans la courbe
d'émission et ne retarde pas les récompenses.

### 4.2 Émission décroissante

L'émission suit une série géométrique en temps réel. **La moitié de l'offre (500 M) est émise
pendant les 20 premières années**, la moitié du reste pendant les 20 suivantes, et ainsi de
suite. Dans chaque période, le rythme est constant : environ **0,79 VINX par seconde**, soit
~9,5 VINX par bloc, pendant les 20 premières années.

| Échéance | Émis au total | Restant |
|---|---|---|
| Genèse | 0 | 1 Md |
| 20 ans | 500 M | 500 M |
| 40 ans | 750 M | 250 M |
| 60 ans | 875 M | 125 M |
| ∞ | → 1 Md | → 0 |

L'émission dépend de l'horodatage des blocs : un réseau arrêté n'émet rien, et un réseau
rapide n'émet pas plus.

### 4.3 Répartition (preuve d'enjeu, ADR 0086)

```
Émission, à chaque bloc  ──►  20 % au proposeur du bloc
                         ──►  80 % à la cagnotte d'époque
Frais, à chaque bloc     ──►  50 % au proposeur du bloc
                         ──►  50 % à la cagnotte d'époque
Cagnotte, chaque heure   ──►  aux validateurs actifs, selon  enjeu × présence
```

- **Enjeu** : la garantie déposée, plafonnée à 10 % de l'enjeu actif dès 10 validateurs.
- **Présence** : blocs co-signés divisés par blocs que le validateur pouvait co-signer, sur
  7 jours glissants.

Engager plus rapporte plus ; être absent rapporte moins, jusqu'à rien. Rien n'est jamais
brûlé : émission et frais ne font que changer de main. Quand l'émission devient négligeable,
les frais prennent naturellement le relais.

### 4.4 Frais

- **Forfaitaires** : un paiement coûte **0,0001 VINX**, quel que soit son montant.
- **Congestion** : le prix de base monte quand les blocs se remplissent et redescend ensuite.
- **Garantie, retrait, réhabilitation** : sans frais.

### 4.5 Invariant

À chaque bloc, et vérifié par chaque nœud :

```
circulation + cagnotte d'époque + poussière détruite = total émis ≤ 1 000 000 000 VINX
```

Un bloc qui violerait cet invariant est rejeté.

---

## 5. Paiements

- **Mémo** : 32 octets signés avec le paiement, pour une référence de facture ou un numéro de
  commande. Un compte peut exiger un mémo (`SetMemoRequired`), ce qui évite les dépôts
  impossibles à attribuer.
- **Adresses lisibles** : `nom@domaine` est résolu par un fichier publié sur le site du
  domaine, sans registre sur la chaîne.
- **Reçus** : la transaction, l'en-tête du bloc et la preuve d'inclusion. N'importe qui peut
  les vérifier sans nœud, même après l'élagage.
- **Finalité** : un paiement inclus est définitif ; le commerçant peut livrer au bloc suivant.

---

## 6. Réseau sans centre

- **Découverte** : les nœuds se trouvent par Kademlia et mémorisent leurs pairs. Les points
  d'entrée (seeds), embarqués dans le logiciel, ne servent qu'à arriver ; le réseau survit à
  leur disparition.
- **Box internet** : un nœud tente d'ouvrir son port (UPnP). S'il reste injoignable, il le
  détecte (AutoNAT), passe par deux relais et tente une liaison directe (DCUtR). Valider ne
  demande aucune configuration réseau.
- **Résistance** : messages bornés, limitation de débit par pair et bannissement **temporaire**
  (5 minutes) : un pair bruyant est mis à l'écart sans jamais être coupé définitivement.
- **Genèse vérifiée** : la genèse officielle est publiée avec le logiciel ; un nœud refuse d'en
  rejoindre une autre.

---

## 7. Accessibilité

- **Application de bureau** (Windows, macOS, Linux) : portefeuille chiffré, phrase de
  récupération de 12 mots, envoi avec référence, réception par QR code, et un **interrupteur
  « Valider »** qui lance le nœud intégré, dépose la garantie et suit l'échauffement.
- **Ligne de commande** : `./vinx join` rejoint le réseau, demande des VINX de test, dépose la
  garantie et lance l'échauffement, en une seule commande.
- **Alertes** sur le téléphone en cas d'arrêt, d'isolement ou de suspension.
- **SDK TypeScript** et API HTTP pour les intégrations marchandes.

---

## 8. Gouvernance et évolution

VinX suit le modèle de Linux (ADR 0081) : le mainteneur publie le **logiciel**, chaque
validateur choisit la version qu'il exécute. Le mainteneur ne dirige pas la **chaîne**.

### 8.1 Mises à jour sans redémarrage (ADR 0086)

1. Une nouvelle version est publiée. Chaque nouvelle règle y est conditionnée à la version
   active du protocole.
2. La mise à jour est annoncée sur la chaîne avec un préavis : **7 jours** pour un correctif,
   **30 jours** pour une version mineure, **90 jours** pour une version majeure.
3. Chaque bloc indique la version que fait tourner son proposeur.
4. À l'échéance, la mise à jour s'active **seulement si plus des 2/3 de la puissance de vote
   l'a installée**. Sinon, elle reste en attente : les validateurs ont un droit de veto de fait.
5. Un nœud resté sur une ancienne version s'arrête proprement et le signale. Il n'applique
   jamais des règles qu'il ne connaît pas.

Les anciens blocs restent valides avec les anciennes règles : **la chaîne ne repart jamais de
zéro**.

### 8.2 Clé de lancement

Pendant les **365 premiers jours** seulement, une clé d'administration peut annoncer les mises
à jour et ajuster les paramètres gouvernables (frais de base, garantie minimale dans ses
bornes). Ce pouvoir s'éteint définitivement à l'échéance : c'est une constante du protocole.
Son remplacement par une annonce votée par les validateurs est prévu avant le mainnet.

Il n'existe **aucun gel de compte** : la propriété des VINX est inconditionnelle.

---

## 9. Règles immuables

1. **1 milliard de VINX au maximum**, jamais augmenté.
2. **Aucune pré-allocation sur le mainnet** : tout est émis par le fonctionnement du réseau.
3. **Courbe d'émission fixée à la genèse** : demi-vie de 20 ans, en temps réel.
4. **Rien n'est brûlé** (hors poussière des comptes vidés, au plus 0,001 VINX par compte).
5. **Invariant d'offre** vérifié à chaque bloc.
6. **Finalité immédiate** : un bloc certifié par plus des 2/3 ne peut pas être annulé.
7. **Propriété inconditionnelle** : aucun compte ne peut être gelé.
8. **Rail de paiement** : pas de VM, pas de contrats intelligents, pas d'exécution de code tiers.

---

## 10. Positionnement

**VinX est :**
- un rail de paiement : de la valeur qui circule vite, définitivement et de façon vérifiable ;
- un protocole assez petit pour être audité par une personne attentive ;
- un réseau que chacun peut faire tourner chez soi.

**VinX n'est pas :**
- une plateforme de DeFi ou de contrats intelligents ;
- un concurrent des chaînes généralistes ;
- un produit spéculatif : pas de prévente, pas de promesse de rendement.

---

## 11. Feuille de route

**Fait**
- Consensus BFT, finalité immédiate, état léger, reçus vérifiables
- Preuve d'enjeu, entrée libre, set stable, suspension et réhabilitation automatiques
- Réseau sans serveur central, passage des box (UPnP, relais, DCUtR)
- Mises à jour sans redémarrage, activées par les validateurs
- Application de bureau avec validateur en un clic, alertes, kit de lancement du testnet

**En cours**
- Testnet public (chaîne 7), premiers validateurs indépendants, tests d'endurance
- Page de paiement commerçant et portefeuille mobile

**Avant le mainnet**
- Audit de sécurité externe
- Annonce des mises à jour par vote des validateurs, en remplacement de la clé d'administration
- Applications signées, genèse et points de contrôle publiés
- Lancement équitable, sans pré-allocation

VinX n'a pas de pression d'agenda : chaque étape est franchie quand elle est prête.

---

## Annexe : synthèse

| Catégorie | Valeur |
|---|---|
| **Type** | Rail de paiement L1, sans VM, un seul token (VINX) |
| **Consensus** | BFT Tendermint, BLS12-381 agrégé, finalité immédiate (> 2/3) |
| **Bloc** | 12 s · jusqu'à 3 000 transactions |
| **Historique** | 30 jours (état permanent compact, `--archive` en option) |
| **Offre** | 1 Md VINX, plafond immuable, aucune pré-allocation sur le mainnet |
| **Émission** | Demi-vie de 20 ans, temps réel · ~9,5 VINX par bloc au départ |
| **Récompenses** | 20 % au proposeur · 80 % selon enjeu × présence · frais 50/50 |
| **Validateurs** | Garantie min. 1 000 VINX · échauffement 3 h · set actif ≤ 100 · poids plafonné à 10 % |
| **Sanctions** | Absence : suspension · double vote : 5 % + 3 × part corrélée · déliaison 21 jours |
| **Frais** | 0,0001 VINX par paiement, ajustés à la congestion |
| **Paiements** | Mémo de 32 octets, `nom@domaine`, reçus hors ligne |
| **Réseau** | Kademlia, UPnP, AutoNAT, relais, DCUtR |
| **Mises à jour** | Activées à > 2/3 des validateurs, sans redémarrage · préavis 7/30/90 jours |
| **Gouvernance** | Clé de lancement de 365 jours, puis aucune · aucun gel de compte |

---

*VinX Labs, octobre 2026 — document de référence v7.0*
