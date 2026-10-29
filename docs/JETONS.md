# Créer et gérer un jeton sur VinX Ledger

Depuis le protocole 5, chacun peut émettre son propre jeton : points de
fidélité, bons d'achat, crédits, monnaie d'une association, et à terme une
monnaie stable. Il n'y a pas de contrat intelligent à écrire : le jeton est une
fonction native de la chaîne, avec des règles connues et vérifiées par tous les
nœuds (SPEC §5.11).

Les frais de chaque opération restent payés en VINX. Les exemples utilisent la
ligne de commande `vinx-wallet` ; l'onglet « Jetons » du portefeuille arrive au
lot 5c.

## 1. Réserver le symbole

Le symbole d'un jeton est un nom `.vinx` que vous possédez, en majuscules :
`eurv.vinx` donne `EURV`. Personne ne peut donc créer un jeton à votre nom.

```bash
vinx-wallet name register --key emetteur.json eurv.vinx
```

Un symbole ne sert qu'une fois : même si le nom change de mains ou expire plus
tard, le jeton existant le garde et personne ne peut en créer un second.

## 2. Créer le jeton

```bash
vinx-wallet asset create --key emetteur.json --symbol EURV --name "Euro VinX" \
  --decimals 2 --supply 10000 --mintable --freezable --clawback
```

- `--decimals` : de 0 (unités entières, des points par exemple) à 9.
- `--supply` : quantité créditée à l'émetteur dès la création.
- `--max-supply` : plafond définitif, si vous en voulez un.
- La création coûte 100 VINX (paramètre `asset_fee`, réglable par la
  gouvernance), versés à la trésorerie.

### Les pouvoirs de l'émetteur

Ils sont choisis à la création, affichés à tous, et **ne peuvent ensuite
qu'être abandonnés, jamais ajoutés** :

| Option | Pouvoir | Pour qui |
|---|---|---|
| `--mintable` | Émettre de nouvelles unités | Monnaie stable, points distribués au fil de l'eau |
| `--freezable` | Geler un compte, ou tout le jeton | Émetteur régulé, réponse à une fraude |
| `--clawback` | Reprendre des unités sur un compte | Émetteur régulé (décision de justice, erreur) |

Sans aucune option, le jeton a une supply fixe et l'émetteur n'a plus aucun
pouvoir dessus. Un pouvoir s'abandonne définitivement :

```bash
vinx-wallet asset renounce --key emetteur.json --symbol EURV --clawback
```

## 3. Distribuer

On ne reçoit un jeton qu'après l'avoir accepté. Cela protège chacun contre les
jetons non sollicités (arnaques, spam) :

```bash
vinx-wallet asset accept --key client.json --symbol EURV            # le client
vinx-wallet asset send --key emetteur.json --symbol EURV --to vinx1… --amount 25.50
vinx-wallet asset mint --key emetteur.json --symbol EURV --to vinx1… --amount 100
```

L'accord coûte une transaction au client. L'émetteur peut en payer les frais à
sa place (frais payés par un parrain, SPEC §5.10). `asset leave` retire un
jeton de son compte quand le solde est nul.

## 4. Consulter

```bash
vinx-wallet asset list                  # tous les jetons
vinx-wallet asset show EURV             # émetteur, supply, pouvoirs
vinx-wallet asset balances vinx1…       # soldes d'une adresse
```

API du nœud : `/assets`, `/assets/{symbole}`, et le champ `assets` de
`/accounts/{adresse}`. Les montants y sont en unités du jeton (`units`,
`supply_units`) : divisez par 10^`decimals`.

## 5. Sécuriser l'émission

Une seule clé qui peut émettre sans limite, c'est un risque. Mettez des règles
de compte (§9 du guide du portefeuille) sur le compte émetteur : avec une
politique, émettre, geler, reprendre ou abandonner un pouvoir exige
**toujours** les approbations des signataires, même sous le plafond journalier.

```bash
vinx-wallet account set-policy --key emetteur.json \
  --signer CLÉ-1 --signer CLÉ-2 --signer CLÉ-3 --threshold 2
```

## 6. Monnaie stable : ce que la chaîne fait, et ce qu'elle ne fait pas

La chaîne fournit les outils : émission et destruction contrôlées, gel,
reprise forcée, compte émetteur multisig, et preuves de réserves publiées par
horodatage (`Attest`).

Elle ne garantit pas la valeur du jeton : c'est l'émetteur qui détient les
réserves. Dans l'Union européenne, émettre une monnaie stable adossée à l'euro
relève du règlement MiCA (agrément d'établissement de crédit ou de monnaie
électronique). Sur le testnet, vous pouvez tout essayer librement.
