# Portefeuille mobile

Le portefeuille mobile sert à **payer et être payé** depuis un téléphone :
solde, envoi (VINX ou jeton), réception par QR code, scan d'un QR de paiement,
historique, acceptation d'un jeton. Les fonctions avancées (création de jetons,
séquestres, mandats, multisig) restent dans le portefeuille de l'ordinateur.

## Sécurité

- Les clés ne quittent jamais le téléphone. La phrase de 12 mots y est chiffrée
  par ton mot de passe (PBKDF2-SHA-256, 600 000 tours, puis AES-GCM).
- La signature est faite par le même code Rust que la chaîne, compilé en
  WebAssembly : une transaction signée sur le téléphone est identique à celle
  du portefeuille de bureau.
- **Les mêmes 12 mots donnent les mêmes comptes** sur le téléphone et sur
  l'ordinateur.
- Le portefeuille se verrouille après 5 minutes en arrière-plan.
- Le nœud ne reçoit que des transactions déjà signées.

## Installer sur Android

1. Sur le téléphone, ouvre la page des releases du projet sur GitHub et
   télécharge `vinx-X.Y.Z-android.apk`.
2. Ouvre le fichier. Android demande d'autoriser l'installation depuis le
   navigateur (« sources inconnues ») : accepte pour cette installation.
3. Ouvre **VinX**, puis crée un portefeuille ou restaure tes 12 mots.
4. La première fois que tu touches **Scanner**, autorise l'accès à la caméra.

Le nœud utilisé par défaut est `https://vinx-ledger.vinxlabs.fr` (Réglages
pour en changer).

**Mettre à jour.** Pour l'instant, l'APK est signée avec une clé de test qui
change à chaque version : Android refuse alors la mise à jour par-dessus
l'ancienne. Désinstalle l'ancienne version, installe la nouvelle, puis
restaure ton portefeuille avec tes 12 mots. **Vérifie que tu les as notés
avant de désinstaller.**

## Essayer sur un ordinateur

```bash
crates/vinx-mobile/build.sh
python3 -m http.server -d crates/vinx-mobile/www 8090
```

puis ouvre <http://127.0.0.1:8090/> et, dans Réglages, l'adresse de ton nœud.
La caméra ne fonctionne que sur `localhost` ou en HTTPS.

## Pour les développeurs

- `crates/vinx-mobile/src/lib.rs` : phrase, dérivation des comptes, signature,
  QR codes (affichage et lecture), en WebAssembly. La dérivation est partagée
  avec le portefeuille de bureau.
- `crates/vinx-mobile/www/` : l'interface (HTML et JavaScript, sans dépendance).
- `crates/vinx-mobile/capacitor/` : l'emballage Android (Capacitor).
  `prepare.sh VERSION` génère le projet, puis `cd android && ./gradlew
  assembleDebug`. La CI le fait dans `.github/workflows/android.yml`.
- Le nœud autorise les appels d'autres origines (CORS) : l'API est publique et
  sans cookie.
