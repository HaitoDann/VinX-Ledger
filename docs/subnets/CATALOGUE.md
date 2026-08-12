# Catalogue des subnets VinX

- **Statut :** vivant (backlog d'idées, non figé)
- **Lié :** infrastructure de subnets (ADR 0039) ; émission élastique (ADR 0040) ; répartition
  par l'usage (ADR 0041) ; garde-fous d'équité (ADR 0044) ; doctrine « monnaie pure + modules
  hors-nœud » (ADR 0001).

> Ce document liste des idées de subnets — soit à construire par **🏭 VinX Labs**, soit à
> proposer à la **🌱 communauté** pour qu'elle orchestre son propre nœud. Ce n'est **pas** une
> décision (pas un ADR) : c'est un backlog qui évolue.

## Le filtre unique : « qui paie, et pourquoi en VINX ? »

Une idée n'entre dans le catalogue que si on peut **nommer le client payeur** et dire **pourquoi
il a besoin de VINX**. Sans demande réelle, pas de melt, pas d'émission légitime (ADR 0041/0044).
Corollaire : **VinX ne juge jamais la valeur du travail** — elle est révélée par ce que le client
accepte de payer (le melt). La L1 mesure un flux, pas une qualité.

**Deux conditions transverses de succès grand public :**
1. **UX en fiat, VINX en coulisse.** Le grand public ne détiendra pas de crypto pour utiliser un
   service ; une passerelle carte → VINX → melt doit être invisible côté utilisateur.
2. **Un vrai moat.** « Payer en VINX » ajoute de la friction vs les incumbents ; le service doit
   être soit nettement **moins cher** (capacité dormante), soit **uniquement possible en
   décentralisé** (résistance à la censure, diversité géographique, confidentialité).

**Deux rails de rémunération** (ADR 0044) : le **paiement direct** (client → mineurs, prévisible,
permanent) fait vivre le service ; l'**émission dirigée par le melt** est une subvention
d'amorçage bornée, qui décroît quand la Fonderie se vide.

## Gabarit par subnet

Chaque idée est décrite par : *Service rendu · Client payeur · Mesure de valeur · Amorçage ·
Mineurs & scoring · Bond / anti-Sybil · Dépendances & risques · 🏭/🌱 · Effort.*

---

## Pool retenu (4)

### 1. 🟢 Monitoring / uptime distribué
- **Service rendu :** surveiller la disponibilité et la latence de sites/APIs depuis de nombreux
  points du globe (« Pingdom décentralisé »).
- **Client payeur :** propriétaires de sites, apps, PME (marché SaaS établi : Pingdom, UptimeRobot).
- **Mesure de valeur :** le client melte du VINX au prorata du nombre de sondes / de la fréquence.
- **Amorçage :** offre gratuite limitée + VinX Labs opère les premières sondes ; la demande vient
  des devs qui veulent de la surveillance multi-région.
- **Mineurs & scoring :** un agent léger fait des requêtes et remonte des mesures ; réputation +
  consensus entre sondes (une sonde qui ment est écartée), `reward_root` de l'opérateur.
- **Bond / anti-Sybil :** bond de sonde modeste ; croisement des mesures détecte les fausses.
- **Dépendances & risques :** faible surface ; risque = collusion de sondes (mitigé par consensus).
- **🏭 Labs (priorité)** — moat *intrinsèque* au décentralisé (vantage points réels), mineur trivial.
- **Effort :** faible.

### 2. 🟢 Stockage décentralisé
- **Service rendu :** stocker et restituer des fichiers de façon redondante et vérifiable.
- **Client payeur :** grand public (sauvegarde), apps ayant besoin de stockage bon marché.
- **Mesure de valeur :** melt au prorata des Go·mois réellement payés par les clients.
- **Amorçage :** VinX Labs stocke ses propres données + offre d'appel ; briques réutilisées par
  les autres subnets (contenu, etc.).
- **Mineurs & scoring :** partage de disque dormant ; **preuves de stockage/récupération**
  périodiques ; `reward_root` adossé au bond (fraude → slash, ADR 0023).
- **Bond / anti-Sybil :** bond par capacité annoncée ; preuves d'inclusion (ADR 0034).
- **Dépendances & risques :** ADR 0034 (preuves), ADR 0023 (fraude) ; risque = perte de données
  (mitigé par redondance + preuves).
- **🏭 Labs (priorité, ancre)** — brique de base des services de données.
- **Effort :** moyen.

### 3. 🟡 Calcul partagé (build/CI) — avec modèle « Qubic-like »
- **Service rendu :** exécuter des builds/tests CI et des jobs de calcul pour des équipes de dev.
- **Client payeur :** équipes de développement (les minutes CI type GitHub Actions coûtent cher).
- **Mesure de valeur :** melt au prorata des minutes de calcul réellement consommées.
- **Modèle Qubic-like (revenu de secours) :** quand les machines n'ont **pas de tâche client**,
  elles minent un **protocole externe** (ex. Monero) ; **Montage 2** — VinX Labs encaisse ce
  revenu externe (finance l'infra) et **paie les mineurs en VINX depuis sa trésorerie**. Le
  protocole **ne frappe jamais** de VINX pour du travail externe (ADR 0044). L'infra est ainsi
  rentable même à vide.
- **Mineurs & scoring :** CPU (et GPU) dormants ; résultats de build vérifiables (reproductibles) ;
  `reward_root`.
- **Bond / anti-Sybil :** bond par capacité ; vérification des résultats (reproductibilité).
- **Dépendances & risques :** dépendance à un protocole externe pour le revenu de secours
  (volatilité, régulation — à diversifier) ; isolation/sandbox des jobs clients.
- **🏭 Labs** — finance l'infra via la trésorerie accumulée (validateurs + revenu externe).
- **Effort :** moyen-élevé.

### 4. 🟢 Annotation IA (mineurs = humains)
- **Service rendu :** produire des données étiquetées pour l'entraînement d'IA (boîtes sur
  images, transcription, modération, comparaison de réponses / RLHF).
- **Client payeur :** entreprises qui entraînent des modèles (marché des données d'entraînement,
  ex. Scale AI — énorme et récurrent).
- **Mesure de valeur :** melt au prorata des tâches livrées et acceptées par le client.
- **Amorçage :** VinX Labs apporte les premiers jeux de tâches / clients ; onboarding **zéro
  matériel** → afflux rapide de mineurs.
- **Mineurs & scoring :** n'importe quel humain avec un navigateur ; **qualité par consensus**
  (même tâche à plusieurs, croisement) + réputation + bond anti-triche ; `reward_root`.
- **Bond / anti-Sybil :** petit bond + réputation ; les réponses aberrantes sont écartées et
  pénalisées.
- **Dépendances & risques :** qualité/triche (mitigée par redondance + consensus + bond).
- **🌱 Communauté / 🏭 Labs** — fort attrait communautaire (« gagne du VINX depuis ton canapé »).
- **Effort :** moyen (côté plateforme).

---

## Idées en réserve (non retenues pour l'instant)

Écartées **non** parce que mauvaises, mais pour garder le focus ou parce qu'elles supposent
d'**acheter du matériel dédié** (contrainte : ne miner qu'avec ce qu'on a déjà) :

- **Contenu à l'acte / micropaiement** (article, vidéo « pay-per-view ») — *killer app* grand
  public et gros générateur de demande VINX ; le paiement est **natif L1**, le subnet ne gère que
  l'hébergement/diffusion (réutilise le stockage). À reconsidérer en priorité pour la suite.
- **VPN / proxy résidentiel** (connexion existante) — grand public, mais juridiquement sensible
  (nœuds de sortie).
- **CDN / diffusion**, **transcodage vidéo**, **inférence IA/GPU**, **rendu 3D** — solides mais
  redondants en ressources avec le pool actuel.
- **DePIN physique** (énergie, réception radio SDR, capteurs, couverture sans-fil) — moat le plus
  fort mais **exige d'acheter du matériel** → écartées par choix.
- **Sous-titrage/traduction**, **panels de sondage** — bons compléments « humains », à ajouter si
  on étoffe l'offre.
