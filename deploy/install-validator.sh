#!/usr/bin/env bash
# Installe un validateur VinX Ledger sur un serveur Debian ou Ubuntu.
# Voir docs/DEPLOYER-UN-VALIDATEUR.md.
#
#   sudo ./install-validator.sh prepare
#       Crée l'utilisateur vinx, la clé du validateur (mot de passe aléatoire,
#       stocké dans /etc/vinx/key-password) et la clé réseau. Affiche la clé
#       publique et l'identifiant réseau à reporter dans la genèse et chez les
#       autres nœuds. Sans effet sur ce qui existe déjà.
#
#   sudo ./install-validator.sh start genesis.json [--archive] [--peer ADRESSE]...
#       Installe la genèse, les options et le service, démarre le nœud et
#       vérifie qu'il répond. Relancer la commande remplace les options.
#
# Le binaire vinx-node doit être dans /usr/local/bin ou à côté de ce script.
set -euo pipefail

ETC=/etc/vinx
DATA=/var/lib/vinx
BIN=/usr/local/bin/vinx-node
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

die() { echo "erreur : $*" >&2; exit 1; }
[ "$(id -u)" -eq 0 ] || die "à lancer avec sudo"

install_binary() {
    if [ -x "$HERE/vinx-node" ] && ! cmp -s "$HERE/vinx-node" "$BIN"; then
        install -m 755 "$HERE/vinx-node" "$BIN"
    fi
    [ -x "$BIN" ] || die "binaire introuvable : copie vinx-node dans $BIN ou à côté du script"
}

prepare() {
    install_binary
    id vinx >/dev/null 2>&1 ||
        useradd --system --home "$DATA" --shell /usr/sbin/nologin vinx
    install -d -m 755 "$ETC"
    install -d -o vinx -g vinx -m 700 "$DATA"

    if [ ! -f "$ETC/key-password" ]; then
        (umask 077; head -c 32 /dev/urandom | base64 >"$ETC/key-password")
    fi
    if [ ! -f "$ETC/validator.json" ]; then
        "$BIN" --password-file "$ETC/key-password" keygen --out "$ETC/validator.json" |
            awk '{print $3}' >"$ETC/validator.pub"
        chown vinx:vinx "$ETC/validator.json"
        chmod 600 "$ETC/validator.json"
    fi
    local peer_id
    peer_id="$(runuser -u vinx -- "$BIN" peer-id --data-dir "$DATA")"
    local ip
    ip="$(hostname -I | awk '{print $1}')"

    cat <<EOF

Validateur prêt.
  clé publique       $(cat "$ETC/validator.pub")
  adresse réseau     /ip4/$ip/tcp/26656/p2p/$peer_id

La clé publique va dans la genèse (--validator) ou dans add-validator ;
l'adresse réseau va dans les --peer des autres nœuds.
Sauvegarde hors du serveur : $ETC/validator.json et $ETC/key-password.
EOF
}

start() {
    local genesis="${1:-}"
    [ -n "$genesis" ] && [ -f "$genesis" ] || die "usage : start genesis.json [--archive] [--peer ADRESSE]..."
    shift
    [ -f "$ETC/validator.json" ] || die "lance d'abord : $0 prepare"
    install_binary

    local options=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --archive) options+=(--archive); shift ;;
            --peer) [ $# -ge 2 ] || die "--peer attend une adresse"; options+=(--peer "$2"); shift 2 ;;
            *) die "option inconnue : $1" ;;
        esac
    done

    if [ "$(realpath "$genesis")" != "$ETC/genesis.json" ]; then
        if [ -f "$ETC/genesis.json" ] && ! cmp -s "$genesis" "$ETC/genesis.json"; then
            die "$ETC/genesis.json existe et diffère : un nœud ne change pas de chaîne"
        fi
        install -m 644 "$genesis" "$ETC/genesis.json"
    fi
    echo "VINX_OPTIONS=\"${options[*]}\"" >"$ETC/vinx-node.env"
    install -m 644 "$HERE/systemd/vinx-node.service" /etc/systemd/system/vinx-node.service
    systemctl daemon-reload
    systemctl enable vinx-node >/dev/null 2>&1
    systemctl restart vinx-node

    echo "Démarrage…"
    local status=""
    for _ in $(seq 1 30); do
        status="$(curl -fsS http://127.0.0.1:8545/status 2>/dev/null)" && break
        sleep 1
    done
    [ -n "$status" ] || die "le nœud ne répond pas ; voir : journalctl -u vinx-node -n 50"
    echo "$status"
    echo
    echo "Nœud démarré. Suivi : journalctl -u vinx-node -f"
}

case "${1:-}" in
    prepare) prepare ;;
    start) shift; start "$@" ;;
    *) die "usage : $0 prepare | start genesis.json [--archive] [--peer ADRESSE]..." ;;
esac
