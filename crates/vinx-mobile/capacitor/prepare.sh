#!/bin/sh
# Prépare le projet Android du portefeuille mobile : compile le cœur en
# WebAssembly, génère le projet Capacitor, ajoute la permission caméra (scan
# des QR codes) et fixe la version. Ensuite :
#   cd android && ./gradlew assembleDebug
# Prérequis : Node.js, Java 17+, le SDK Android (ANDROID_HOME), et ceux de
# ../build.sh.
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
version="${1:?usage : prepare.sh VERSION (par exemple 0.19.0)}"

"$here/../build.sh"
cd "$here"
npm ci --no-audit --no-fund
rm -rf android
npx cap add android

manifest=android/app/src/main/AndroidManifest.xml
sed -i 's|<uses-permission android:name="android.permission.INTERNET" />|<uses-permission android:name="android.permission.INTERNET" />\n    <uses-permission android:name="android.permission.CAMERA" />\n    <uses-feature android:name="android.hardware.camera" android:required="false" />|' "$manifest"
grep -q 'android.permission.CAMERA' "$manifest"

# 0.19.0 → 1900 : chaque version publiée a un numéro plus grand que la précédente.
code=$(echo "$version" | awk -F. '{ print $1 * 10000 + $2 * 100 + $3 }')
sed -i "s/versionCode 1$/versionCode $code/; s/versionName \"1.0\"/versionName \"$version\"/" android/app/build.gradle
grep -q "versionName \"$version\"" android/app/build.gradle
echo "projet Android prêt : version $version ($code)"
