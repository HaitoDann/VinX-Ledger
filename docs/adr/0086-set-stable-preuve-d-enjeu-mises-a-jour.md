# ADR 0086 — Set de validateurs stable, preuve d'enjeu et mises à jour sans redémarrage

- **Statut :** Accepté ✅ — implémenté
- **Date :** Octobre 2026
- **Portée :** Protocole (récompenses, set de validateurs, version), nœud, outils
- **Décideur :** VinX Labs (mainteneur)
- **Principe directeur :** le réseau ouvert doit tenir face à des validateurs qui arrivent
  et partent n'importe quand, récompenser l'engagement, et évoluer sans jamais repartir
  de la genèse.

---

## 1. Récompenses proportionnelles à l'enjeu (preuve d'enjeu)

- La cagnotte d'époque (80 % de l'émission et la moitié des frais) est partagée entre les
  validateurs actifs selon **enjeu × présence** :
  - **enjeu** : la garantie déposée, en VINX. À partir de 10 validateurs, elle est plafonnée
    à 10 % de l'enjeu actif total, comme le poids de vote ;
  - **présence** : blocs co-signés ÷ blocs que le validateur pouvait co-signer, sur la
    fenêtre glissante. Un validateur absent touche moins, jusqu'à rien.
- Les 20 % d'émission versés au proposeur de chaque bloc sont inchangés. Le proposeur est
  tiré selon le poids de vote : en moyenne, ce versement suit lui aussi l'enjeu.
- **Garantie minimale du mainnet : 1 000 VINX** (au lieu de 10 000). C'est aussi le
  plancher gouvernable (`MIN_BOND_HARD_FLOOR`).

## 2. Un set qui ne change pas brutalement

Avec un consensus BFT, la chaîne s'arrête si plus d'un tiers de la puissance disparaît
**en même temps**. Ce choix est voulu : la chaîne s'arrête plutôt que de risquer un fork,
et repart quand les validateurs reviennent. Les mesures ci-dessous rendent ce cas très
improbable.

- **Entrées plafonnées** : au plus 10 % du set (au minimum 1) de nouveaux validateurs par
  époque, les mieux classés d'abord. Les autres attendent en réserve (« Benched ») et
  entrent aux époques suivantes.
- **Sorties plafonnées** : au plus 2 sorties volontaires par époque (ADR 0036, inchangé).
- **Les absents quittent le vote tout de suite** : un validateur suspendu pour blocs
  manqués (3 de ses tours de proposition, ADR 0027) sort du set de vote **au bloc
  suivant**, et non plus à la fin de l'époque. Le quorum ne le compte plus. Des absences
  étalées dans le temps ne s'additionnent donc jamais jusqu'au tiers fatal. Simulation :
  3 validateurs sur 7 tombent l'un après l'autre, la chaîne continue.
- **Retour automatique** : revenu en ligne et le délai de suspension écoulé, le nœud envoie
  lui-même `Unjail` avec sa clé d'opérateur, même si ce compte n'a pas de fonds. Le
  validateur réintègre le set à la fin d'époque suivante, dans la limite du plafond
  d'entrées.
- **Toujours en place** : échauffement de 3 époques, déblocage de la garantie en 21 jours,
  classement du set actif (100 au plus) par présence.

## 3. Mises à jour sans redémarrer la chaîne

- **Signal** : chaque en-tête de bloc porte `version`, la version du protocole que fait
  tourner le logiciel du proposeur (`NODE_PROTOCOL_VERSION`). Le champ fait partie du hash
  du bloc.
- **Annonce** : une mise à jour est annoncée sur la chaîne, avec un préavis (7 jours pour
  un correctif, 30 jours pour une version mineure). C'est aujourd'hui l'administrateur qui
  l'annonce ; la clé d'administration expire après 365 jours (ADR 0081).
- **Activation** : à la date prévue, **et seulement si** des validateurs détenant plus des
  2/3 de la puissance de vote ont signalé une version au moins égale. Sinon, la mise à
  jour reste en attente. Les validateurs ont donc un droit de veto de fait, et personne
  n'est coupé du réseau par une version que la majorité n'a pas installée.
- **Ancien logiciel** : un nœud dont la chaîne a activé une version qu'il ne connaît pas
  refuse d'exécuter les blocs. Il ne diverge pas silencieusement. `/health` indique
  `upgrade_required`, et `./vinx status` comme les alertes le signalent.
- **Règles conditionnées** : toute nouvelle règle de consensus s'écrit
  `if state.protocol_at_least(x, y, z) { nouvelle règle } else { ancienne règle }`. Un nœud
  à jour rejoue l'ancien historique avec les anciennes règles. Plus de reset.

### Procédure de mise à jour

1. Écrire la nouvelle règle derrière `protocol_at_least`, puis incrémenter
   `NODE_PROTOCOL_VERSION`.
2. Publier la version (tag `v…`) : binaires et installateurs sont construits
   automatiquement.
3. Annoncer la mise à jour : `vinx-wallet announce-upgrade --version X.Y.Z
   --activation-ts <date>`.
4. Les validateurs installent la nouvelle version. Leurs blocs la signalent.
5. Activation dès que la date est passée et que plus des 2/3 de la puissance est prête.

## Conséquence

Le format des blocs change (nouveau champ `version`). Le testnet officiel part donc d'une
nouvelle genèse. C'est le **dernier** redémarrage : les évolutions suivantes passent par
la procédure ci-dessus.
