# ADR 0064 — VinX = rail de paiement uniquement (périmètre produit)

- **Statut :** Décidé
- **Date :** Septembre 2026
- **Portée :** Produit — périmètre fonctionnel du L1
- **Décideur :** VinX Labs (Haitodann)
- **Supersède (périmètre) :** ADR 0001 (modules L1), ADR 0010 (ancrage modules), ADR 0024 (subnets/escrow), ADR 0034 (DA Celestia), ADR 0048 (ForceExit), ADR 0049 (Clearinghouse), ADR 0050 (SP1 ZK)
- **Crates :** Aucun — contrainte de périmètre, pas de code à ajouter.

---

## 1. Contexte

Le protocole core de VinX est complet : consensus PoA + BLS, VRF, compact blocks, snapshot sync, parallel sync. On se trouve à la frontière entre « code qui tourne en dev » et « réseau qui tourne en public ».

À ce stade, deux directions sont possibles :

**A — L1 généraliste :** VM, modules d'Appchain, vérification ZK (SP1), Clearinghouse cross-chain, ForceExit, DA Celestia. Surface très large, comparaisons directes avec Ethereum/Cosmos/Solana inévitables.

**B — Rail de paiement pur :** Envoyer des VinX rapidement, de façon fiable et vérifiable. Point. La VM et les modules restent hors scope jusqu'à preuve de besoin post-testnet.

## 2. Décision

**Option B retenue.** VinX est un rail de paiement L1 minimaliste, auditable, sans VM.

Les types de transactions actuels couvrent le cas d'usage complet :

| Type | Usage |
|------|-------|
| `Transfer` | Paiement natif VinX |
| `Bond` / `Unbond` | Staking validateur |
| `ValidatorJoin` / `ValidatorExit` | Gestion du set de validateurs |
| `FeeAdjust` | Gouvernance des frais |

De nouveaux types de transaction peuvent être ajoutés sans VM si un besoin est prouvé après testnet. La décision sera documentée dans un nouvel ADR.

## 3. Ce qui est explicitement hors scope

Les ADRs suivants décrivent des fonctionnalités en dehors du périmètre de la v1 :

| ADR | Fonctionnalité | Statut révisé |
|-----|----------------|---------------|
| ADR 0001 | Modules L1 / Appchains | Hors scope — pas de VM |
| ADR 0010 | Ancrage bondé modules | Hors scope |
| ADR 0024 | Subnets, escrow, récompenses opérateurs | Hors scope |
| ADR 0034 | DA Celestia | Hors scope |
| ADR 0048 | ForceExit escape hatch | Hors scope |
| ADR 0049 | Clearinghouse cross-appchain | Hors scope |
| ADR 0050 | Vérification preuves SP1/ZK | **Abandonné** — pas de ZK dans un rail de paiement |

Ces ADRs ne sont pas supprimés (ils documentent une décision passée), mais ils sont **gelés** : aucune implémentation ne sera engagée sur ces fonctionnalités avant que le testnet démontre un besoin réel et que la décision soit réexaminée dans un nouvel ADR.

## 4. Pourquoi cette décision

| Argument | Développement |
|----------|---------------|
| **Surface d'attaque réduite** | Moins de code = moins de bugs = auditabilité totale |
| **Message produit clair** | « Envoyer des VinX vite et sûrement » est compréhensible sans whitepaper |
| **Pas de comparaison directe** | Un rail de paiement n'est pas comparé à Ethereum ou Solana |
| **Performances prévisibles** | Sans VM ni modules, le profil de performance est linéaire et auditable |
| **Testnet d'abord** | Lancer un réseau public est plus urgent qu'implémenter des features non demandées |
| **Abandon du ZK** | SP1 ajoute une dépendance externe critique, une latence de proving, et un audit Groth16 coûteux — pour un rail de paiement qui n'a pas de smart contracts à prouver, c'est de la complexité gratuite |

## 5. Conséquences

- `PROTOCOL_SPEC.md`, `whitepaper.md`, `README.md` doivent être mis à jour pour supprimer les références aux modules, Appchains et ZK.
- Le répertoire `sdk/vinx-appchain-sp1/` peut être archivé.
- Les ADRs 0001 à 0050 touchant aux modules/ZK sont conservés comme historique mais marqués `Gelé — hors scope ADR 0064` dans leur en-tête si modifiés.
- Tout futur ADR proposant des modules/ZK doit référencer ADR 0064 et démontrer un besoin avéré post-testnet.
