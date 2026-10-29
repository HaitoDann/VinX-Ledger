# Le portefeuille VinX

Le portefeuille sert à envoyer et recevoir des VINX, à réserver un nom
`.vinx` et à payer par séquestre. Il tourne sur ton ordinateur : tes clés n'en
sortent jamais.

## 1. Installer et ouvrir

1. Télécharge l'archive de ton système dans la dernière release GitHub :
   - Windows : `vinx-X.Y.Z-windows-x86_64.zip` ;
   - macOS : `vinx-X.Y.Z-macos-arm64.tar.gz`.
2. Décompresse-la.
3. Double-clique sur `vinx-wallet.exe` (Windows). Sous macOS ou Linux, lance
   `./vinx-wallet` dans un terminal.

Une petite fenêtre noire s'ouvre, puis le portefeuille apparaît dans ton
navigateur, à l'adresse `http://127.0.0.1:8700/`.

- **Garde la fenêtre noire ouverte** tant que tu utilises le portefeuille.
  La fermer quitte le portefeuille.
- Windows peut afficher « Windows a protégé votre ordinateur » : clique sur
  « Informations complémentaires », puis « Exécuter quand même ».

Le portefeuille est enregistré dans `%APPDATA%\VinX\portefeuille.json` sous
Windows. Sous macOS, c'est `~/Library/Application Support/VinX/`, et sous Linux
`~/.local/share/vinx/`.

## 2. Créer le portefeuille

1. **Créer un portefeuille.** Douze mots s'affichent : c'est ta **phrase de
   récupération**.
   - Recopie-la sur papier, dans l'ordre.
   - Range-la en lieu sûr.
   - Ne la photographie pas, ne l'envoie à personne.
2. Le portefeuille te demande trois des mots, pour vérifier ta copie.
3. **Choisis un mot de passe**, de 8 caractères au moins. Il protège le
   portefeuille sur cet ordinateur.
4. Indique l'**adresse du nœud** donnée par l'opérateur du réseau, par exemple
   `https://noeud.exemple.fr` pour un nœud public, ou `http://127.0.0.1:8545`
   à travers un tunnel SSH.

À retenir :

- La phrase de 12 mots suffit à tout récupérer, sur n'importe quel ordinateur,
  avec « J'ai déjà une phrase de récupération ».
- Le mot de passe seul ne suffit pas : sans la phrase, un ordinateur perdu, ce
  sont des VINX perdus.
- Quiconque possède la phrase peut prendre tes VINX. Personne de VinX ne te la
  demandera jamais.

Le portefeuille se verrouille après 15 minutes sans activité, ou avec le bouton
« Verrouiller ».

## 3. Recevoir

L'onglet **Recevoir** affiche ton adresse (`vinx1…`) et un bouton pour la
copier.

Un compte qui n'a encore jamais rien reçu doit d'abord recevoir au moins
0,1 VINX : c'est la réserve minimale du réseau.

**Demander un paiement.** Indique un montant, une référence et un motif : le
portefeuille crée un lien `vinx:…` et son QR code. Envoie le lien, ou montre
le QR. La personne qui paie colle le lien dans l'onglet Envoyer, et tout se
remplit.

## 4. Envoyer

Si tu as reçu une demande de paiement (lien `vinx:…`), colle-la dans le premier
champ : le destinataire, le montant et la référence se remplissent.

Dans l'onglet **Envoyer**, le destinataire peut être :

- une adresse `vinx1…` ;
- le nom d'un de tes **contacts** ;
- un nom **`.vinx`**, par exemple `alice.vinx`.

Sous le champ, le portefeuille affiche l'adresse trouvée. Vérifie-la.

**Vérifier avant d'envoyer** ouvre un aperçu, et rien ne part sans ta
confirmation. L'aperçu montre :

- le destinataire et la façon dont il a été trouvé ;
- le montant, les frais du réseau et le total débité ;
- le solde qui te restera ;
- des mises en garde : adresse absente de tes contacts, destinataire qui n'a
  encore jamais reçu de VINX…

Certaines erreurs sont refusées avant même l'aperçu :

- solde insuffisant ;
- montant trop petit pour un nouveau compte ;
- mémo exigé par le destinataire ;
- solde restant sous la réserve.

**Envoi groupé** (protocole 4) : pour payer jusqu'à 100 personnes d'un coup,
colle une ligne par paiement dans le cadre « Envoi groupé » :
`destinataire ; montant ; mémo`. Le mémo est facultatif, et les lignes qui
commencent par `#` sont ignorées. Les colonnes copiées d'un tableur
fonctionnent aussi. L'aperçu montre le total et les mises en garde ; les frais
sont d'un frais minimal par tranche de 10 paiements.

Une transaction est confirmée en quelques secondes. Elle apparaît d'abord « en
attente » dans l'historique.

## 5. Noms `.vinx`

Un nom remplace ton adresse : on te paie en tapant `tonnom.vinx`.

- **Règles :** 3 à 32 caractères, lettres minuscules, chiffres et tirets.
- **Prix :** fixé par le réseau, 10 VINX par an au départ.
- **Renouvellement :** à faire avant l'échéance. L'année s'ajoute à celle en
  cours.
- **Après l'échéance :** tu as encore 30 jours pour renouveler. Ensuite, le
  nom redevient libre.
- **Cession :** « Céder » donne le nom à quelqu'un d'autre.

## 6. Séquestres

Le séquestre protège un achat entre deux personnes qui ne se connaissent pas.

1. L'acheteur ouvre un séquestre : le montant est bloqué, ni lui ni le vendeur
   ne peuvent y toucher.
2. Ensuite, l'un de ces cas se produit :
   - le colis arrive : l'acheteur clique sur « Verser au vendeur » ;
   - le vendeur ne peut pas livrer : il clique sur « Rembourser l'acheteur » ;
   - il y a un litige : l'**arbitre**, s'il a été désigné à l'ouverture,
     décide de verser ou de rembourser ;
   - personne n'agit : à l'échéance, l'acheteur peut se rembourser lui-même.

Chacun voit ses séquestres en cours dans l'onglet **Séquestres**, avec son rôle
(acheteur, vendeur, arbitre).

## 7. Horodater un document

L'onglet **Horodatage** prouve qu'un fichier existait à une date donnée et n'a
pas été modifié depuis : contrat, devis, création, photo…

1. Choisis le fichier. Le portefeuille calcule son empreinte (SHA-256) ; le
   fichier ne quitte pas ton ordinateur.
2. Valide : l'empreinte est inscrite sur la chaîne, pour le prix des frais.
3. Une fois l'opération confirmée, télécharge le **certificat**. Garde-le avec
   le fichier : il contient l'identifiant de la transaction.

Pour vérifier, n'importe qui choisit le fichier dans « Vérifier un
horodatage ». Depuis le protocole 4, l'empreinte suffit : le nœud retrouve
l'horodatage tout seul. Pour un horodatage fait avant, colle aussi
l'identifiant de transaction du certificat. La moindre modification du
fichier change l'empreinte, et la vérification échoue.

Un nœud ordinaire ne garde les blocs que 24 h : pour vérifier un horodatage
ancien, il faut un nœud archive (`--archive`).

## 8. Exporter l'historique

Le bouton **Exporter en CSV** de l'onglet Historique crée un fichier lisible
par Excel ou LibreOffice : date, type, contrepartie, montant, frais, mémo,
bloc et transaction.

## 9. Plusieurs comptes

**Réglages → Nouveau compte** crée un autre compte, par exemple « Épargne ».

Tous les comptes viennent de la même phrase de 12 mots : une restauration les
retrouve tous. Pour cela, recrée les comptes dans le même ordre ; ils
reprennent les mêmes adresses.

## 10. Si quelque chose ne va pas

| Message | Que faire |
|---|---|
| Nœud injoignable | Vérifie l'adresse du nœud dans Réglages, et le tunnel SSH s'il y en a un. |
| Port 8700 déjà utilisé | Le portefeuille est déjà ouvert : cherche sa fenêtre noire ou l'onglet du navigateur. |
| Fonction pas encore active | Les noms et les séquestres arrivent avec le protocole 2, que les administrateurs du réseau doivent activer. |
| Mot de passe oublié | Supprime `portefeuille.json`, puis restaure avec ta phrase de 12 mots. |

## Pour les curieux

Les comptes sont dérivés de la graine BIP-39 de la phrase, sans phrase
secrète :

```
clé du compte i = BLAKE3-derive_key("VinX Ledger portefeuille v1 compte ed25519", graine ‖ i en u32 petit-boutiste)
```

La phrase est chiffrée sur le disque, comme les fichiers de clé : Argon2id puis
XChaCha20-Poly1305. La page web ne reçoit jamais ni la phrase ni les clés,
seulement des adresses, des montants et des aperçus. Elle n'accepte que les
requêtes portant le jeton de la session, venues de `127.0.0.1`.
