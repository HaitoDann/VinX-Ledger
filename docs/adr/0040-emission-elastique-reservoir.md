# ADR 0040 — Émission élastique à réservoir

- **Statut :** Proposé (design constitutionnel — à décider avant le mainnet)
- **Catégorie :** Tokenomics · **Priorité :** 🔴 haute
- **Date :** Août 2026
- **Remplace :** la courbe fixe à halving de l'ADR 0021 (voir amendement 0021).
- **Lié :** immutabilité de l'émission (ADR 0021, amendée) ; répartition inter-subnet par
  l'usage (ADR 0041) ; infrastructure de subnets (ADR 0039) ; invariant de masse (ADR 0004) ;
  heartbeat/distribution (ADR 0038) ; genèse fair launch (ADR 0033).

> ⚠️ **Décision constitutionnelle, fenêtre unique.** La loi d'émission est **immuable** après
> lancement (ADR 0021). Ce document remplace la courbe à halving discret *avant* le mainnet.
> C'est une refonte de la politique monétaire, pas un réglage.

---

## 1. Contexte — pourquoi remplacer la courbe fixe

Deux problèmes de la courbe actuelle (halving discret, cf. code `cumulative_emission_atoms`) :

1. **Elle ne distribue qu'aux validateurs.** Toute l'émission va au producteur de bloc → les
   validateurs thésaurisent la totalité des jetons neufs, le grand public n'a aucun moyen d'en
   obtenir sans les leur acheter. C'est le **problème de distribution** de fond (cf. discussions
   0039/0041) : on veut que *tout le monde* puisse gagner des VINX en travaillant dans les
   subnets, les dépenser, les garder, ou bonder pour lancer son propre subnet.
2. **Elle est rigide et déconnectée de l'usage.** Débit plat par ère + falaises aux années
   8/16/24 ; le montant émis ne tient aucun compte de l'activité réelle du réseau.

On veut une politique qui **distribue largement aux mineurs des subnets**, **s'auto-régule sur
l'usage réel**, reste **hard-cappée** et **immuable dans sa loi**, sans comité ni spéculation.

## 2. Décision — l'émission est un réservoir à débit proportionnel

Avec l'invariant de masse `C + F = MAX` (C = circulation, F = Fonderie, MAX = 100 Md) :

> **Débit d'émission `E = r · F`** — on émet une fraction fixe `r` de ce qui reste dans la
> Fonderie, par unité de temps. Le **melt** (VINX consommés en échange d'un service) **revient
> dans la Fonderie** (recyclage, jamais détruit — l'invariant l'exige).

- **`r = 7 %/an`** (constante immuable — voir §5 pour le choix).
- Plus la Fonderie est pleine → plus l'émission est forte. Plus on a émis → moins il reste →
  moins on émet. Plus le melt est élevé → la Fonderie se remplit → l'émission remonte.
- L'émission émise à chaque bloc alimente les **reward pools des subnets** (ADR 0039), répartie
  entre subnets **au prorata du melt** (ADR 0041). Les **validateurs** ne vivent plus de
  l'émission mais des **frais** (couche consensus, cf. modèle à deux couches).

### Dynamique

```
dC/dt = E − M = r·(MAX − C) − M
```

où `M` est le débit de melt (usage réel). Le melt joue donc **deux rôles** : il **remplit la
Fonderie** (règle le niveau global d'émission, ici) et il **pondère la répartition** entre
subnets (ADR 0041).

## 3. Propriétés (démontrées par simulation — `scripts/emission_sim.py`)

### Auto-régulation vers un équilibre stable

Pour un usage `M` donné, le système converge (linéaire, stable, constante de temps `1/r`) vers :

```
C* = MAX − M/r        F* = M/r        E* = M
```

**À l'équilibre, l'émission égale le melt** (`E* = M`) : les mineurs gagnent, collectivement,
*exactement ce que le réseau est utilisé*. La Fonderie n'est qu'un **tampon** entre « les gens
consomment » et « les mineurs sont payés ».

### Circulation hard-cappée ET élastique

La circulation converge vers `C* < MAX` par le bas — elle **ne dépasse jamais MAX** (jamais
inflationniste), tout en étant élastique à la demande sous le cap.

### L'amorçage est gratuit (résout le dilemme Sybil de 0041)

Au lancement, `F = MAX` (Fonderie pleine) → `E = r·MAX` est fort **même sans aucun usage**. La
Fonderie **bootstrap la distribution elle-même** : les mineurs sont payés et les jetons se
répandent *avant* que l'usage existe, puis le melt prend le relais. On n'a donc **pas besoin**
d'un `r` élevé pour amorcer.

### Cas dégénéré sûr

Si le réseau n'a **jamais** d'usage (`M = 0`), le modèle dégénère proprement en une
**exponentielle décroissante** classique : la Fonderie se vide, `C → MAX`, `E → 0`. Pas de mode
d'échec — au pire une distribution unique, au mieux une émission auto-entretenue.

### Prévisibilité

La **loi** (`E = r·F`) est déterministe et immuable ; seul le débit *réalisé* varie avec
l'usage — comme l'ajustement de difficulté de Bitcoin (règle fixe, sortie variable).
L'argument de confiance « nul ne change la création monétaire » tient pleinement.

## 4. Chiffres à `r = 7 %/an` (usage réaliste `M ∝ C`, m = 15 %/an)

| Grandeur | Valeur |
|---|---|
| Constante de temps `1/r` | 14,3 ans |
| Circulation d'équilibre `C*` | ≈ 31,8 Md VINX (Fonderie ≈ 68,2 Md) |
| Émission d'équilibre `E* = M*` | ≈ 4,77 Md VINX/an |
| 90 % de `C*` atteint à | ≈ 10,4 ans |
| Distribué en année 1 (**sans usage**) | ≈ 6,34 Md VINX |

`r = 7 %` a été retenu comme compromis : front-loading modéré (bon pour le fair launch — laisse
le temps à la base de mineurs de s'élargir avant que le gros soit distribué), tout en gardant un
amorçage confortable (~6,3 Md dès l'an 1, financé par le tampon Fonderie). Voir la comparaison
`r = 5/7/10/20 %` dans le simulateur.

## 5. Ce qui reste immuable (inchangé)

- **Cap et invariant** : `C + F = MAX = 100 Md` (ADR 0004).
- **Aucun pre-mine, aucune allocation fondateur.**
- **La loi `E = r·F` et `r = 7 %`** deviennent les nouvelles constantes gravées (ADR 0021).

## 6. Conséquences

**Positif**
- **Distribue les jetons à tout le monde** (mineurs des subnets), pas aux seuls validateurs —
  règle le problème de fond.
- **S'auto-régule** sur l'usage réel ; supprime les falaises de halving ; aligne les mineurs sur
  la génération d'usage réel.
- **Amorçage gratuit** via le tampon Fonderie ; **cas sans usage** sûr.
- Circulation **hard-cappée** et non-inflationniste.

**Coûts / pièges**
- **Refonte constitutionnelle** : remplace la courbe à halving déjà implémentée (et son test
  constitutionnel). À porter en code dans une tranche dédiée (voir §8).
- **Émission réalisée variable** (mitigé : loi fixe, cf. §3).
- **Wash-farming du melt** à border au niveau de la *répartition* (ADR 0041 : plafond +
  gate-bond) — pas au niveau de la loi globale ici.
- Choix de `r` **irréversible** après lancement.

## 7. Alternatives écartées

- **Courbe fixe à halving (statu quo).** Rejeté : ne distribue qu'aux validateurs, rigide,
  déconnectée de l'usage.
- **Émission gouvernable / ajustable.** Rejeté (ADR 0021) : réintroduit une politique monétaire
  discrétionnaire.
- **`E = r·F` avec `r` élevé (≥ 20 %).** Rejeté : ~17 Md dès l'an 1 → trop front-loadé →
  concentration (le risque même du fair launch).
- **Melt qui détruit les jetons** (au lieu de recycler vers la Fonderie). Rejeté : casse
  l'invariant `C + F = MAX` (0004). Le melt **doit** retourner à la Fonderie.

## 8. Notes d'implémentation (tranche future)

- Remplacer `cumulative_emission_atoms` (halving discret) par le débit `E = r · F` intégré sur le
  temps réel (timestamps, comme aujourd'hui ; cf. ADR 0005/0038). Arithmétique **entière pure**
  (pas de flottant — consensus) : approximer `r·F·Δt` par calcul entier borné, vecteur doré.
- Introduire le **melt** comme flux `circulation → Fonderie` (le README actuel dit « plus de
  melt » — on le **réintroduit** comme signal d'usage ; cf. ADR 0041 pour la mécanique de
  consommation d'un service).
- Mettre à jour le **test constitutionnel** de l'ADR 0021 : il n'épingle plus le halving mais la
  loi `E = r·F` et `r = 7 %`.
- L'émission alimente les reward pools des subnets (ADR 0039), pas le producteur ; les
  validateurs sont crédités des **frais** au règlement du bloc (inchangé pour les frais).
- Simulateur de référence : `scripts/emission_sim.py`.
