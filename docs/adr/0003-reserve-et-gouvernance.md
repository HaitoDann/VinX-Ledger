# ADR 0003 — Réserve du protocole et gouvernance votée (réseau principal)

- **Statut :** accepté (principes) ; valeurs à confirmer avec un avis juridique
  avant la genèse du réseau principal
- **Date :** 2026-11-02
- **Remplace, pour le réseau principal :** l'escrow et les paiements de
  trésorerie par la gouvernance (ADR 0001 §§2–4, ADR 0002 §1 ligne
  « Escrow »)
- **Ne change pas :** l'émission des validateurs (ADR 0002 : 100 M, demi-vie
  10 ans) et les frais aux validateurs

## Contexte

Aujourd'hui, 900 M de VINX (90 % de la supply) sont libérés de l'escrow par
tranches mensuelles **par les trois clés admin**, qui décident aussi de leur
usage. C'est le premier motif de méfiance qu'opposeront investisseurs,
régulateurs et partenaires : la mise en circulation et l'usage de la quasi
totalité de la supply sont **subjectifs**, à la main du fondateur.

Une recherche sur les projets comparables (XRP, Stellar, Hedera, Zcash,
Decred, Dash, Polkadot, Cosmos, Optimism ; cadre MiCA) montre :

- une libération **automatique et prévisible** inspire confiance (XRP, Hedera,
  Zcash) ;
- un fondateur qui décide seul des dépenses est le reproche majeur (ventes de
  Ripple, réserve de la fondation Stellar, détruite à 55 Md en 2019 faute
  d'usage) ;
- le **vote collectif des dépenses, plafonné**, tient le mieux dans la durée
  (Decred : vote des détenteurs, plafond mensuel ; Dash : vote des
  masternodes) ;
- une distribution **gratuite** reste hors des obligations d'offre au public
  de MiCA (article 4(3)), tant qu'aucune cotation n'est annoncée ; une
  cotation exigera un livre blanc MiCA.

Les validateurs sont déjà rémunérés par l'émission (100 M) et les frais
(ADR 0002) : la réserve de 900 M ne sert **pas** à les payer.

## Décisions

### 1. Une réserve gérée par le protocole

- À la genèse du réseau principal, **900 000 000 VINX** forment la **réserve
  du protocole**. Il n'y a plus de tranches, d'escrow ni de trésorerie
  alimentée par l'admin.
- Aucune clé ne contrôle la réserve. Elle ne sort que par une **demande
  acceptée** (§2).
- **Au plus 5 000 000 VINX par mois** (mois de 2 629 800 s). Le non-dépensé
  reste dans la réserve et **ne se reporte pas** : le plafond ne s'accumule
  jamais.
- **Au plus 100 000 VINX par demande.** Une demande plus grande se découpe en
  plusieurs demandes, chacune votée.
- Au rythme maximal, la réserve couvre au moins 15 ans.
- La réserve **ne vote pas**, et elle entre dans l'invariant de supply.

### 2. Dépenser : demande, vote des validateurs, vote des détenteurs

1. **Demande.** Tout compte peut déposer une demande : bénéficiaire, montant
   (au plus 100 000 VINX), objet, lien vers un descriptif public. Une
   **caution** (paramètre `proposal_deposit`) protège contre le spam. Elle est
   rendue si la demande obtient le soutien minimal, et sinon versée à la
   réserve.
2. **Vote des validateurs** (7 jours) : la demande passe si **plus des 2/3 des
   validateurs actifs** l'approuvent, comme pour les blocs. Un validateur dont
   l'adresse est bénéficiaire ne compte pas.
3. **Vote des détenteurs** (14 jours) : la demande est validée si :
   - la **participation** atteint au moins **10 % des VINX en circulation**
     (réserve, budget d'émission non émis et comptes du protocole exclus) ;
   - et si les **voix pour dépassent les voix contre**, au poids des VINX.

   Voter **bloque** les VINX jusqu'à la fin du scrutin, pour qu'on ne puisse
   pas voter deux fois en les déplaçant.
4. **Versement**, dans la limite du plafond mensuel. Sinon, la demande attend
   le mois suivant, par ordre d'acceptation. Les gros projets sont **versés
   par étapes** : une demande par étape.

Les clés admin n'ont **aucun pouvoir** sur la réserve.

### 3. Paramètres : proposés par la gouvernance, votés

| Type | Exemples | Vote |
|---|---|---|
| **Économiques** | `min_fee`, `min_reserve`, `object_reserve`, `name_fee`, `asset_fee`, `validator_bond`, `min_availability`, `vote_share`, `proposal_deposit` | Validateurs (plus des 2/3) **puis** détenteurs (quorum de 10 %, majorité) |
| **Techniques** | délais de consensus, `min_block_interval`, `max_block_txs`, `max_block_bytes`, `max_memo_bytes`, `absence_blocks` | Validateurs seuls (plus des 2/3) |

- **Seule la gouvernance (multisig admin) peut proposer** un changement de
  paramètre, pour éviter les propositions malveillantes (frais à zéro…).
  Elle ne peut plus rien **imposer**.
- Les frais exigent le vote des deux : les validateurs, payés par les frais,
  ont intérêt à les augmenter, et les utilisateurs l'intérêt inverse.

### 4. Ce qui reste à la gouvernance admin

- **Mises à jour du protocole** et **ajout ou retrait de validateurs** :
  décidés par la multisig admin, avec le **délai de 48 h** du protocole 7,
  publiés et annulables pendant ce délai.
- **Urgence** : un correctif de sécurité peut être programmé avec un préavis
  réduit. Il doit être publié et justifié dans les 7 jours.
- La gouvernance **ne peut pas** dépenser la réserve, créer des VINX, ni
  changer un paramètre sans vote.

### 5. Équipe et VinX Labs

- **Aucune part réservée** à l'équipe ou aux fondateurs.
- VinX Labs se rémunère comme tout opérateur, par ses validateurs (émission et
  frais). Il peut déposer, comme tout le monde, des demandes ponctuelles à la
  réserve (audit, juridique, infrastructure), soumises au même double vote.

### 6. Transparence

L'explorateur et l'API publient :

- le solde de la réserve et le plafond restant du mois ;
- chaque demande, avec ses votes ;
- chaque versement.

C'est un rapport automatique, sans document à maquiller.

### 7. Ce qui ne change pas

- **Supply fixe** : 1 milliard.
- **Émission des validateurs** : 100 M, demi-vie 10 ans, plus les frais
  (ADR 0002).
- **Testnet** : il garde l'escrow et les règles actuelles, conformément à
  l'engagement de l'ADR 0001 de ne jamais modifier l'escrow par mise à jour.
  Ce modèle s'applique à **la genèse du réseau principal**, après avoir été
  éprouvé sur un devnet puis sur un testnet dédié.

## Conséquences

- **Positives** :
  - plus aucune part de la supply n'est à la discrétion du fondateur ;
  - un calendrier maximal prévisible ;
  - des dépenses collectives, publiques et plafonnées ;
  - pas de vente publique au lancement ;
  - un récit simple : « la réserve appartient au réseau ».
- **Négatives** :
  - le financement de VinX Labs dépend de ses validateurs et des votes ;
  - un risque de faible participation des détenteurs, d'où le quorum ;
  - les gros détenteurs pèsent plus (vote au poids), contrebalancés par le
    vote des validateurs ;
  - un module de gouvernance important à écrire, auditer et tester.

## À trancher avant la genèse du réseau principal

- Montant de `proposal_deposit` et soutien minimal pour récupérer la caution.
- Quorum de 10 % : à ajuster selon la distribution réelle de la supply au
  lancement. Au tout début, très peu de VINX circulent : quorum calculé sur la
  circulation, ou sur un minimum fixe ?
- Distribution initiale de VINX hors validateurs, par exemple une première
  demande gratuite aux participants du testnet, et compatibilité avec MiCA.
- Avis juridique sur l'ensemble : statut du VINX, de la réserve et du vote.

## Mise en œuvre

Après le protocole 7 : un module de gouvernance (demandes, deux scrutins,
blocage des votes, quorum, versements plafonnés), testé sur devnet, puis sur
un testnet dédié qui part d'une genèse au nouveau modèle, et intégré à la
genèse du réseau principal.
