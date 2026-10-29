# ADR 0002 — Émission des validateurs et frais aux validateurs

- **Statut :** accepté
- **Date :** 2026-10-29
- **Remplace en partie :** ADR 0001 (consensus sans récompenses, trésorerie de 100 M à la genèse)

## Contexte

L'ADR 0001 réservait 100 M de VINX à une trésorerie dès la genèse, versait
tous les frais à cette trésorerie et ne rémunérait pas les validateurs. Deux
problèmes en découlent, maintenant que VinX vise un réseau de validateurs
connus mais indépendants (feuille de route, §1) :

1. **Rien n'incite à faire tourner un validateur.** Un opérateur supporte un
   serveur, du temps et une responsabilité, sans contrepartie.
2. **La mise en circulation dépend entièrement de la gouvernance.** Une réserve
   de 100 M disponible dès la genèse, plus l'escrow, sous le contrôle de trois
   clés, est le premier motif de méfiance.

`VinX-Ledger-Legacy` émettait toute la supply aux validateurs (courbe
exponentielle à demi-vie de 10 ans, aucune pré-allocation, preuve d'enjeu). On
reprend sa courbe, pas sa preuve d'enjeu ni son absence de réserve.

## Décisions

### 1. Répartition de la supply (inchangée : 1 milliard, plafond définitif)

| Part | Montant | Mise en circulation |
|---|---|---|
| Escrow | 900 M | 120 tranches mensuelles de 7,5 M, libérées par la gouvernance (ADR 0001, inchangé) |
| Émission des validateurs | 100 M | Automatique, selon la courbe ci-dessous |
| Trésorerie à la genèse | **0** | Elle ne reçoit plus que les libérations d'escrow |

Rien n'est créé au-delà du milliard : le budget non encore émis fait partie de
l'invariant (SPEC §4.3).

### 2. Courbe d'émission

`émis(t) = 100 M × (1 − 2^(−t / 10 ans))`, où `t` est le temps écoulé depuis la
genèse, mesuré sur l'horodatage des blocs (pas sur leur hauteur), et une année
compte 365,25 jours.

| Échéance | Émis | Rythme |
|---|---|---|
| Lancement | 0 | ~18 980 VINX par jour |
| 1 an | 6,7 M | ~17 700 par jour |
| 10 ans | 50 M | ~9 500 par jour |
| 20 ans | 75 M | ~4 700 par jour |
| 40 ans | 93,75 M | ~1 190 par jour |

La courbe est asymptotique : elle tend vers 100 M sans les atteindre, ce qui
laisse une récompense résiduelle (« poussière ») que les frais doivent relayer.
Le calcul se fait en arithmétique entière (virgule fixe à 10⁻¹⁸) pour être
identique sur toutes les machines.

### 3. Répartition par époque

- Une époque dure `epoch_blocks` blocs : 17 280 par défaut, soit environ 24 h
  à 5 s par bloc. La valeur est fixée à la genèse.
- Chaque bloc est compté pour son **proposeur**, inscrit dans l'en-tête.
  Comme le proposeur tourne à tour de rôle, un validateur présent propose sa
  part, et un validateur absent est sauté.
- Au dernier bloc de l'époque :
  - l'émission due depuis la dernière répartition s'ajoute à la cagnotte ;
  - la cagnotte est répartie entre les validateurs **au prorata des blocs
    proposés** dans l'époque ;
  - le reliquat d'arrondi reste dans la cagnotte.
- Les récompenses sont créditées à l'adresse dérivée de la clé du validateur.

Récompenser les blocs proposés, plutôt que les signatures, évite de changer le
format des blocs : les signatures du bloc précédent n'y sont pas inscrites.

### 4. Frais

À partir du protocole 3, **les frais vont à la cagnotte d'époque**, donc aux
validateurs, et plus à la trésorerie. Le prix d'un nom `.vinx` reste versé à la
trésorerie : c'est le prix d'un service, pas un frais de transaction.

### 5. Réseaux de test : le robinet

Une genèse de réseau de test (identifiant contenant « testnet » ou « devnet »)
peut doter la trésorerie d'un **robinet**, pour financer les testeurs. Il est
pris sur la **dernière tranche d'escrow**, ce qui laisse intacts le budget des
validateurs et la supply. `devnet` dote 1 M de VINX par défaut ; sur un réseau
principal, le robinet vaut 0.

### 6. Activation

C'est le **protocole 3**. Une genèse standard démarre directement en protocole 3.
Un réseau existant qui passerait en protocole 3 par `ScheduleUpgrade` verrait
ses frais aller aux validateurs, mais avec un budget d'émission nul : le budget
ne se fixe qu'à la genèse. Le testnet est donc remis à zéro.

## Conséquences

- Les VINX entrent en circulation en rémunérant un travail réel, sans réserve
  discrétionnaire à la genèse.
- La récompense est un **défraiement**, pas un revenu attractif : environ 6,7 M
  la première année, à partager entre les validateurs. En PoA, la sécurité
  vient de l'identité des validateurs, pas de ce qu'ils gagnent.
- L'escrow (900 M) reste sous le contrôle de la gouvernance. Une politique de
  trésorerie publique reste à écrire.
- Tant que les validateurs appartiennent tous au mainteneur, la récompense lui
  revient. Elle ne prend son sens qu'avec des opérateurs indépendants.
- Le relais par les frais suppose un usage réel : il faut environ 220 tx/s en
  continu pour que les frais égalent l'émission du lancement.

## Refusé

- **Créer des VINX au-delà du milliard** : contraire à l'ADR 0001 et à la promesse
  de supply fixe.
- **Preuve d'enjeu, garanties, sanctions et déliaison de Legacy** : inutiles pour
  un ensemble de validateurs autorisés. Une caution avec sanction en cas de
  double vote pourra venir plus tard.
- **Répartition « enjeu × présence »** : sans enjeu, la présence mesurée par les
  blocs proposés suffit.
