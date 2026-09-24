# ADR 0084 — Staking : désengagement de 21 jours, slashing corrélé, clés séparées

- **Statut :** Accepté ✅ — implémenté
- **Date :** Septembre 2026
- **Portée :** Protocole (bond, slashing, clés de validateur) · nœud · wallet
- **Décideur :** VinX Labs (mainteneur)
- **Modifie :** ADR 0036 (sorties), ADR 0038 (pool), ADR 0046/0075 (clé BLS), ADR 0027 (unjail), ADR 0082 (vérification des certificats)

---

## Décisions

| # | Décision | Valeur |
|---|---|---|
| S1 | Délai de désengagement | **21 jours** (au lieu de 3). Protège contre les attaques à longue portée. Reste inférieur aux 30 jours de rétention des blocs, donc la preuve de toute faute reste disponible. |
| S2 | Âge maximal d'une preuve d'équivocation | **21 jours**, convertis en hauteurs avec le temps de bloc |
| S3 | Pénalité d'équivocation | **5 % + 3 × (part de puissance slashée dans la fenêtre, faute incluse)**, plafonnée à 100 %. Le reliquat repasse par un désengagement complet, pendant lequel il reste slashable. |
| S3 | Exclusion | Définitive : le validateur est retiré du set, du pool et de la file de sortie, et banni de tout nouveau bond |
| S4 | Inactivité | Pas de perte d'argent, seulement la mise à l'écart (jailing) |
| S5 | Clés | Le **propriétaire** (wallet hors serveur) détient le bond, les retraits, les récompenses et les changements de clé. Le **serveur** ne détient que la clé BLS de vote et une clé **opérateur**, autorisée uniquement à faire `unjail`. |
| S6 | Délégation | Aucune dans le protocole |
| S7 | Récompenses | Versées au propriétaire (identité du validateur = adresse du propriétaire) |

Exemples de pénalité S3 :

- un validateur isolé qui détient 1 % de la puissance perd environ 8 % de son bond ;
- avec 5 % de la puissance, il perd 20 % ;
- si un tiers de la puissance fraude, chacun perd 100 %.

## Mise en œuvre

- **Nœud.** `vinx-node --validator-owner <adresse>` (ou `validator_owner` dans la
  configuration) : le nœud vote et propose au nom du propriétaire, avec seulement sa clé
  BLS et sa clé opérateur.
  - `--genesis-entry --validator-owner <adresse>` produit le fichier de clés du
    validateur : clé BLS, preuve de possession (PoP) liée au propriétaire, et adresse de
    l'opérateur.
- **Wallet du propriétaire.**
  - `vinx-wallet stake --validator-keys <fichier>` : bond d'entrée, avec la clé BLS et
    l'opérateur.
  - `vinx-wallet set-validator-keys --keys <fichier>` : rotation de la clé.
- **Wallet de l'opérateur.** `vinx-wallet unjail --validator <adresse>`.
- **État.** Le pool enregistre `operator` pour chaque entrée. Deux champs sont engagés
  dans la racine de consensus :
  - `last_voting_keys` : les clés du set de votants, figées au moment du vote ;
  - `slash_history` : l'historique des slashings, pour la corrélation.

## Bugs existants corrigés

1. **Blocage de chaîne par rotation de clé.** Le certificat d'un bloc était vérifié au
   bloc suivant avec les clés BLS *actuelles* du pool. Deux cas suffisaient à rendre un
   certificat valide invérifiable, et donc à bloquer la chaîne :
   - une rotation de clé incluse dans ce bloc ;
   - un validateur slashé hors du pool dans ce bloc.

   Le correctif vérifie le certificat contre `last_voting_keys`. Un test de régression
   le couvre, et il échoue sur l'ancien code.
2. **Exclusion incomplète.** Un validateur slashé restait dans le pool et pouvait bonder
   de nouveau.
3. **Reliquat libéré trop tôt.** Le reliquat d'un slash était crédité immédiatement. Il
   suit désormais un désengagement complet.
4. **Mauvais nonce pour la dénonciation.** La transaction de dénonciation émise par le
   nœud prenait le nonce du validateur, et non celui de la clé qui la signe.

## Limites

- **Pénalités passées.** Une faute ultérieure n'augmente pas après coup la pénalité des
  fautes précédentes (Ethereum le fait à mi-parcours). La corrélation reste cumulative
  dans l'ordre d'arrivée.
- **Rotation de clé BLS.** Elle prend effet au bloc qui l'inclut. Le nœud doit être
  relancé avec la nouvelle clé, et il manque quelques votes entre-temps.
- **Récompenses.** Elles sont distribuées à la clôture d'époque (1 h), pas à chaque bloc.

## Vérification

- **Tests d'état :**
  - pénalité corrélée : 20 % puis environ 36 % pour deux fautes simultanées ;
  - expiration de la preuve à la limite exacte de la fenêtre ;
  - double slash refusé ;
  - `unjail` accepté de l'opérateur et refusé d'un tiers ;
  - certificat vérifiable malgré une rotation de clé dans son bloc.
- **Simulation complète.** Une équivocation dénoncée par transaction et incluse dans un
  bloc est appliquée à l'identique par les 4 nœuds, puis la chaîne continue à 3.
- **Vrai réseau** (`bench-n4.sh`). node4 tourne **sans la clé du propriétaire** et
  propose des blocs en son nom. Les autres phases (progression, paiement, tolérance,
  arrêt, reprise) passent également. ✅
