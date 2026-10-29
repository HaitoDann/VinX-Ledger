# VinX Ledger 0.18.0

Première version publiée depuis la 0.9, et nouvelle chaîne de test :
`vinx-testnet-2`, au **protocole 5**. Lancement pas à pas : `TESTNET-2.md`,
inclus dans les archives.

## Nouveautés de la chaîne

- **Émission des validateurs** (protocole 3) :
  - 100 M VINX versés sur une courbe à demi-vie de 10 ans ;
  - les frais vont aux validateurs, au prorata des blocs proposés ;
  - la trésorerie de genèse est supprimée, et un robinet finance les testeurs.
- **Paiements et comptes** (protocole 4) :
  - envois groupés (jusqu'à 100 destinataires) ;
  - horodatage de documents ;
  - comptes multisig et plafond journalier ;
  - mandats de prélèvement ;
  - frais payés par un parrain.
- **État en arbres** (protocole 4) : un bloc ne recalcule que les lignes qu'il
  modifie (0,2 ms avec 200 000 noms, au lieu de 500 ms).
- **Jetons natifs** (protocole 5), sans contrat intelligent :
  - le symbole est lié à un nom `.vinx` ;
  - pas de réception sans accord préalable ;
  - les pouvoirs de l'émetteur (émission, gel, reprise forcée) sont déclarés à
    la création, peuvent être abandonnés, jamais ajoutés ;
  - séquestres et abonnements possibles en jetons.

## Nouveautés du logiciel

- **Portefeuille** : onglets Jetons, Mandats, Horodatage, Historique avec export
  CSV, envoi groupé, demande de paiement par QR, règles du compte.
- **Ligne de commande** : `vinx-wallet asset`, `name`, `mandate`, `account`
  (multisig, parrainage).
- **API** : `/assets`, `/assets/{symbole}`, `/attestations/{empreinte}`,
  `/accounts/{a}/transactions`.
- **Script d'installation** : `install-validator.sh reset-chain` pour passer
  sur une nouvelle genèse de test.

## À savoir

- **Changement de chaîne** : les soldes de l'ancien testnet ne sont pas repris.
- Le portefeuille et le cockpit 0.18 sont nécessaires pour le testnet 2.

Documentation : `TESTNET-2.md`, `JETONS.md`, `PORTEFEUILLE.md`,
`EXPLOITATION.md`.
