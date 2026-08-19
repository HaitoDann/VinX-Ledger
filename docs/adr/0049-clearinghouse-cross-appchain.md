# ADR 0049 — Clearinghouse cross-Appchain

- **Statut :** Accepté (design) — non implémenté
- **Catégorie :** Appchains / Interopérabilité · **Priorité :** 🟠 moyenne
- **Date :** Août 2026
- **Liens :** s'appuie sur l'enregistrement des Appchains (ADR 0050) et les state diffs (ADR 0048) ; complémentaire du ForceExit (ADR 0048).

---

## 1. Contexte

Chaque Appchain est une unité isolée : elle génère ses preuves SP1 et les soumet au L1 pour settlement. Mais les actifs et les messages ne peuvent pas naturellement traverser les frontières entre Appchains — leurs états sont indépendants.

Le besoin : permettre à un actif verrouillé sur l'Appchain A d'être restitué sur l'Appchain B, ou à un message d'une Appchain d'être consommé par une autre, sans pont externe ni oracle de confiance.

La contrainte principale est l'**atomicité** : si A brûle un actif mais que B ne minte pas, les tokens disparaissent. Le protocole doit gérer le cas d'échec.

---

## 2. Décision

### 2.1 Architecture : message passing asynchrone via L1

Le L1 joue le rôle de **chambre de compensation (clearinghouse)** entre Appchains. Le transfert cross-chain est **asynchrone** (2 transactions L1, 2 cycles d'ancrage) et non-atomique à l'instant T, mais **garanti à terme** par les mécanismes de timeout et de réclamation.

Pas de tentative d'atomicité synchrone — c'est un problème ouvert dans l'industrie (même Ethereum et Cosmos ne le résolvent pas nativement). L'asynchronisme est assumé.

### 2.2 Flux de transfert A → B

```
┌────────────────────────────────────────────────────────────────────┐
│  1. LOCK sur A                                                      │
│     L'Appchain A brûle/verrouille un actif pour l'adresse dest.    │
│     Elle inclut cette action dans son prochain AnchorState.        │
│     Payload ancré : { type: CrossMsg, id, dest_appchain, amount }  │
└──────────────────────────┬─────────────────────────────────────────┘
                           │ AnchorState avec SP1 proof
                           ▼
┌────────────────────────────────────────────────────────────────────┐
│  2. ENREGISTREMENT sur le L1                                        │
│     Le L1 valide la preuve SP1 d'A.                                │
│     Il enregistre le message cross-chain dans son état :           │
│     CrossMsg { id, src, dst, amount, expiry_ts, status: Pending }  │
└──────────────────────────┬─────────────────────────────────────────┘
                           │ Appchain B lit le L1
                           ▼
┌────────────────────────────────────────────────────────────────────┐
│  3. MINT sur B                                                      │
│     L'Appchain B observe le registre de messages du L1.            │
│     Elle inclut le mint dans son prochain AnchorState.             │
│     Payload ancré : { type: CrossMsgAck, id, status: Delivered }   │
└──────────────────────────┬─────────────────────────────────────────┘
                           │ AnchorState avec SP1 proof
                           ▼
┌────────────────────────────────────────────────────────────────────┐
│  4. CLÔTURE sur le L1                                               │
│     Le L1 marque le message CrossMsg comme Delivered.              │
│     L'action est terminée — le transfert est finalisé.             │
└────────────────────────────────────────────────────────────────────┘
```

### 2.3 Gestion des échecs — timeout et réclamation

Le message a un champ `expiry_ts`. Si B n'émet pas de `CrossMsgAck` avant l'expiry :

1. Quiconque soumet une transaction `CrossMsgExpire` sur le L1 après l'expiry.
2. Le L1 marque le message `Expired`.
3. A peut inclure un `CrossMsgRefund` dans son prochain AnchorState, restaurant le solde verrouillé.

Ce mécanisme garantit que les fonds ne disparaissent jamais : soit B les minte, soit A les récupère.

**Latence du cas nominal :** minimum 2 cycles d'ancrage Appchain (A + B) + 2 blocs L1. En pratique, quelques minutes selon la fréquence d'ancrage.

### 2.4 Registre de messages cross-chain sur le L1

Le L1 maintient un registre borné :

```
cross_msgs: BTreeMap<CrossMsgId, CrossMsg>
```

Avec :
- `CrossMsgId = Hash(src_appchain || dst_appchain || nonce || sender)`
- Purge automatique des messages `Delivered` ou `Expired` après `MSG_RETENTION_BLOCKS` blocs.
- Limite max : `MAX_PENDING_CROSS_MSGS` par paire (src, dst).

### 2.5 Actifs natifs vs actifs wrappés

- **Actifs natifs VINX** : le L1 déplace directement entre les séquestres des Appchains. Garantie maximale — le L1 garde la vérité du stock.
- **Actifs propres à une Appchain** : le transfert repose sur la confiance mutuelle entre Appchains (A brûle, B minte) — aucune garantie L1 sur la valeur de l'actif B. C'est le même modèle que les bridges ERC-20 classiques.

---

## 3. Absence d'atomicité synchrone — choix assumé

Il n'y a **pas de tentative d'atomicité cross-chain synchrone** dans ce design. Les raisons :

1. L'atomicité synchrone requiert une coordination entre les séquenceurs de A et B avant la soumission des preuves — cela crée un couplage fort et une surface d'échec partagée.
2. L'asynchronisme avec timeout + réclamation offre les mêmes garanties de sûreté (les fonds ne disparaissent pas) avec moins de complexité de protocole.
3. Tous les systèmes de bridge production-grade (IBC, LayerZero, Axelar) sont asynchrones.

L'atomicité synchrone peut être construite **par-dessus** ce mécanisme (via un contrat de coordination hors-chaîne ou un séquenceur partagé) sans changer le protocole L1.

---

## 4. Conséquences

**Positif**
- Les actifs ne peuvent pas disparaître (timeout + réclamation garantis).
- Trustless pour les actifs natifs VINX (L1 gère les séquestres).
- Simple à implémenter côté L1 (registre de messages + validateur d'acknowledgement).
- Compatible avec un séquenceur unique par Appchain (V1).

**Coûts / compromis**
- Latence : plusieurs minutes pour un transfert cross-chain nominal.
- Pas d'atomicité synchrone : les DeFi cross-chain complexes (swap atomique) nécessitent des mécanismes applicatifs supplémentaires.
- Le registre de messages est un état L1 borné à gérer.
- B doit monitorer activement le L1 pour détecter les messages à traiter.

---

## 5. Notes d'implémentation

**`vinx-core` :**
- `CrossMsg { id, src_appchain, dst_appchain, sender, recipient, amount, expiry_ts, status }`
- `CrossMsgStatus : { Pending, Delivered, Expired, Refunded }`
- `TransactionType::CrossMsgExpire (0x0C)`

**`vinx-state` :**
- `cross_msgs: BTreeMap<CrossMsgId, CrossMsg>` dans `WorldState`
- `apply_anchor_state()` étendu : parse les `CrossMsg` et `CrossMsgAck` dans les payloads SP1
- `settle_block()` : purge des messages expirés + archivage des livrés

**`vinx-node/rpc` :**
- `GET /cross-msgs?src=&dst=&status=` : liste les messages cross-chain
- `GET /cross-msg/:id` : détail d'un message

**Indexeur (côté Appchain) :**
- Les séquenceurs d'Appchain doivent monitorer `/cross-msgs?dst=<leur_appchain>&status=Pending` et inclure les acknowledgements dans leurs prochains AnchorState.

**Dépendances :** ADR 0050 (SP1 proof verification — les CrossMsg sont parsés depuis les payloads prouvés) ; ADR 0048 (séquestre Appchain — utilisé pour les actifs natifs VINX).
