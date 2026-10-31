# Vérifier une release, et la clé de l'APK

Avant d'installer un binaire sur un validateur, vérifie qu'il vient bien du
dépôt `HaitoDann/VinX-Ledger` et qu'il n'a pas été modifié. Deux contrôles,
du plus simple au plus fort.

## 1. Sommes de contrôle (intégrité)

Chaque release contient `SHA256SUMS`, l'empreinte de chaque fichier.

Linux, dans le dossier où tu as téléchargé l'archive et `SHA256SUMS` :

```bash
sha256sum --ignore-missing -c SHA256SUMS
```

Attendu : `vinx-X.Y.Z-linux-x86_64.tar.gz: OK`.

Windows (PowerShell) : compare la ligne affichée avec celle de `SHA256SUMS`.

```powershell
Get-FileHash .\vinx-X.Y.Z-windows-x86_64.zip -Algorithm SHA256
```

Ce contrôle détecte un fichier abîmé ou remplacé, mais pas un faux
`SHA256SUMS` posé à côté d'un faux binaire.

## 2. Attestation de provenance (authenticité)

Depuis la 0.27, chaque fichier de la release a une **attestation signée par
GitHub** (Sigstore) : elle prouve qu'il a été compilé par le workflow
`release.yml` de ce dépôt, à partir de l'étiquette de la version. Aucune clé
à gérer : la signature est liée au dépôt lui-même.

Avec l'outil `gh` (https://cli.github.com) :

```bash
gh attestation verify vinx-X.Y.Z-linux-x86_64.tar.gz --repo HaitoDann/VinX-Ledger
```

Attendu : `✓ Verification succeeded!`, avec le workflow
`.github/workflows/release.yml` et l'étiquette `refs/tags/vX.Y.Z`.

Les attestations sont aussi visibles sur GitHub : onglet **Actions** →
**Attestations**.

## 3. Clé de l'APK

Android n'accepte une mise à jour que si elle est signée avec la **même clé**
que la version installée. Tant qu'aucune clé n'est configurée, la CI signe
l'APK avec une clé de débogage différente à chaque fois : il faut désinstaller,
réinstaller et restaurer le portefeuille avec ses 12 mots.

### Créer la clé (une seule fois)

Sur une machine de confiance avec Java (par exemple `vinx-public` :
`apt install -y openjdk-17-jre-headless`) :

```bash
keytool -genkeypair -v -keystore vinx-release.keystore -alias vinx \
  -keyalg RSA -keysize 4096 -validity 10000 \
  -dname "CN=VinX Labs, O=VinX Labs, C=FR"
base64 -w0 vinx-release.keystore >vinx-release.keystore.b64
```

`keytool` demande un mot de passe : choisis-en un long et note-le.

### L'enregistrer dans GitHub

Dépôt → **Settings** → **Secrets and variables** → **Actions** →
**New repository secret**, trois fois :

| Nom | Valeur |
|---|---|
| `ANDROID_KEYSTORE_B64` | le contenu de `vinx-release.keystore.b64` (une seule ligne) |
| `ANDROID_KEYSTORE_PASSWORD` | le mot de passe choisi |
| `ANDROID_KEY_ALIAS` | `vinx` |

La release suivante est alors compilée en version « release » et signée avec
cette clé ; l'empreinte du certificat est affichée dans le journal de l'étape
« Compilation et signature de l'APK ».

### Garder la clé

- **Sauvegarde** `vinx-release.keystore` et son mot de passe hors ligne
  (clé USB rangée, gestionnaire de mots de passe). Perdue, plus aucune mise à
  jour ne s'installe par-dessus : tous les utilisateurs devront réinstaller.
- **Ne la diffuse jamais** : qui la détient peut publier une fausse mise à jour
  acceptée par les téléphones.
- Efface les copies de travail (`vinx-release.keystore.b64`) une fois les
  secrets enregistrés.

Le passage de la clé de débogage à la clé fixe impose une **dernière**
réinstallation ; ensuite, chaque version s'installe par-dessus la précédente.
