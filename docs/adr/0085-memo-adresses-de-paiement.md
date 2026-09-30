# ADR 0085 — Mémo de paiement et adresses `nom@domaine`

- **Statut :** Accepté ✅ — implémenté
- **Date :** Septembre 2026
- **Portée :** Protocole (mémo, option de compte), wallet, SDK
- **Décideur :** VinX Labs (mainteneur)
- **Principe directeur :** VinX reste un rail de paiement léger. Rien de ce qui n'est pas
  indispensable au paiement ne va dans l'**état**, qui est permanent. Ce qui va dans les
  **transactions** est élagué après 30 jours (ADR 0083).

---

## Décisions

### Mémo — dans la transaction, 32 octets au plus

- Le mémo est le `payload` d'un transfert. Il est **signé**, **public**, optionnel et ne
  coûte aucun frais supplémentaire.
- Il est élagué avec le bloc après 30 jours et n'a donc **aucun poids à long terme** pour
  les validateurs. Il reste dans le reçu de paiement que conserve le wallet.
- Chaque type de transaction a désormais sa propre limite de `payload` :
  - transfert : 32 octets ;
  - `Unstake` et `Unjail` : 0 octet ;
  - `SetMemoRequired` : 1 octet ;
  - les autres types : 2 Kio (auparavant 16 Kio).
- **Faille corrigée au passage :** un transfert pouvait transporter 16 Kio de données
  arbitraires pour le prix d'un transfert vide. C'était du gonflement de blocs bon marché.

### Mémo obligatoire — une option de compte

- Une nouvelle transaction, `SetMemoRequired` (`0x0D`), signée par le titulaire du compte
  et payée au frais de base, active ou lève l'option.
- Tant que l'option est active, tout transfert sans mémo vers ce compte est refusé. Le
  refus a lieu dès l'admission au mempool, donc l'expéditeur reçoit l'erreur tout de
  suite, et il est répété à l'application du bloc.
- C'est le fonctionnement des adresses de dépôt d'exchange sur Stellar et XRP.
- L'option est stockée dans un ensemble engagé dans la racine de consensus. Il ne
  contient que les comptes qui ont activé l'option, ce qui représente un poids négligeable.

### Noms : **hors chaîne** — adresses `nom@domaine`

- Il n'y a **pas de registre de noms dans le protocole**. Un tel registre serait
  permanent et grossirait l'état sans être nécessaire au paiement : une transaction va
  toujours vers une adresse.
- On paie à `julie@vinxpay.com`. Le wallet lit
  `https://vinxpay.com/.well-known/vinx.json?name=julie`, qui renvoie
  `{"names":{"julie":"vinx1…"}}`. Il **affiche l'adresse obtenue**, puis signe le
  paiement vers **cette adresse**. C'est le modèle de la Lightning Address de Bitcoin et de
  la fédération de Stellar.
- **Garde-fous :**
  - HTTPS obligatoire (sauf `localhost` pour le développement) ;
  - pas de redirection ;
  - nom en ASCII minuscule uniquement, pour éviter les usurpations par caractères
    homoglyphes ;
  - l'adresse reçue est validée avant tout paiement.
- La confiance repose sur le domaine, comme pour l'e-mail. N'importe qui peut servir les
  noms de son propre domaine. VinX Labs peut proposer `@vinxpay.com` comme service.

## Mise en œuvre

- **Wallet :**
  - `vinx-wallet transfer --to julie@vinxpay.com --memo FAC-412` ;
  - `vinx-wallet memo-required on|off`.
- **SDK :** `encodeMemo`, `parsePaymentAddress`, `resolvePaymentAddress`, et les champs
  `memo` et `memo_required` dans les réponses.
- **RPC :** les transactions exposent `memo` (texte) et `memo_hex` ; les comptes exposent
  `memo_required`.

## Vérification

- **Tests d'état :**
  - mémo de 32 octets accepté, 33 refusé ;
  - avec l'option active, un transfert sans mémo est refusé et un transfert avec mémo
    accepté ;
  - lever l'option change la racine d'état, ce qui prouve qu'elle y est engagée ;
  - un `Unstake` avec des données est refusé.
- **Wallet et SDK :** analyse des adresses, avec refus des majuscules, des caractères non
  ASCII et de l'injection de chemin, et résolution avec une requête HTTP simulée.
- **Vrai réseau** (`bench-n4.sh`) : un paiement avec mémo, relu sur un autre nœud. ✅
