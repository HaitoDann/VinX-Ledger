#!/usr/bin/env bash
# Installe un validateur VinX Ledger sur un serveur Debian ou Ubuntu.
# Voir docs/DEPLOYER-UN-VALIDATEUR.md.
#
#   sudo ./install-validator.sh prepare [--observer]
#       Crée l'utilisateur vinx, la clé du validateur (mot de passe aléatoire,
#       stocké dans /etc/vinx/key-password) et la clé réseau. Affiche la clé
#       publique et l'identifiant réseau à reporter dans la genèse et chez les
#       autres nœuds. Sans effet sur ce qui existe déjà.
#       --observer : nœud sans clé, qui suit la chaîne et sert l'API (nœud
#       public, voir docs/NOEUD-PUBLIC.md). Il ne vote jamais.
#
#   sudo ./install-validator.sh start genesis.json [--archive] [--mempool-size N]
#       [--validator-names noms.json] [--node-name NOM] [--peer ADRESSE]... [--allow IDENTIFIANT]...
#       --allow : accepte un nœud qui se connecte à celui-ci (nœud public).
#       Installe la genèse, les options et le service, démarre le nœud et
#       vérifie qu'il répond. Relancer la commande remplace les options.
#
#   sudo ./install-validator.sh upgrade [nouveau-vinx-node]
#       Met à jour le binaire (par défaut celui à côté du script) : sauvegarde
#       le binaire et les données, redémarre, et revient à l'ancien binaire si
#       le nœud ne finalise pas de nouveau bloc dans les 2 minutes.
#
#   sudo ./install-validator.sh rollback
#       Revient au binaire d'avant la dernière mise à jour.
#
#   sudo ./install-validator.sh reset-chain nouvelle-genesis.json
#       Réseau de test seulement : passe le nœud sur une nouvelle genèse.
#       Sauvegarde puis efface les données de l'ancienne chaîne, en gardant
#       l'identité réseau (les adresses --peer et --allow ne changent pas),
#       la clé du validateur et les options.
#
# Le binaire vinx-node doit être dans /usr/local/bin ou à côté de ce script.
set -euo pipefail

ETC=/etc/vinx
DATA=/var/lib/vinx
BIN=/usr/local/bin/vinx-node
BACKUP=/var/backups/vinx
API=http://127.0.0.1:8545
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
    local observer=""
    case "${1:-}" in
        "") ;;
        --observer) observer=1 ;;
        *) die "usage : prepare [--observer]" ;;
    esac
    install_binary
    id vinx >/dev/null 2>&1 ||
        useradd --system --home "$DATA" --shell /usr/sbin/nologin vinx
    install -d -m 755 "$ETC"
    install -d -o vinx -g vinx -m 700 "$DATA"

    local peer_id ip
    peer_id="$(runuser -u vinx -- "$BIN" peer-id --data-dir "$DATA")"
    ip="$(hostname -I | awk '{print $1}')"
    if [ -n "$observer" ]; then
        [ -f "$ETC/validator.json" ] &&
            die "ce serveur a déjà une clé de validateur : un observateur s'installe sur un serveur à part"
        touch "$ETC/observer"
        cat <<EOF

Observateur prêt (aucune clé : il ne vote pas).
  identifiant réseau  $peer_id

Sur chaque validateur, relance « start » en ajoutant : --allow $peer_id
EOF
        return 0
    fi
    [ -f "$ETC/observer" ] &&
        die "ce serveur est préparé en observateur (supprime $ETC/observer pour en faire un validateur)"

    if [ ! -f "$ETC/key-password" ]; then
        (umask 077; head -c 32 /dev/urandom | base64 >"$ETC/key-password")
    fi
    if [ ! -f "$ETC/validator.json" ]; then
        "$BIN" --password-file "$ETC/key-password" keygen --out "$ETC/validator.json" |
            awk '{print $3}' >"$ETC/validator.pub"
        chown vinx:vinx "$ETC/validator.json"
        chmod 600 "$ETC/validator.json"
    fi

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
    [ -n "$genesis" ] && [ -f "$genesis" ] || die "usage : start genesis.json [--archive] [--peer ADRESSE]... [--allow ID]..."
    shift
    local unit=vinx-node.service
    if [ -f "$ETC/observer" ]; then
        unit=vinx-node-observer.service
    else
        [ -f "$ETC/validator.json" ] || die "lance d'abord : $0 prepare (ou prepare --observer)"
    fi
    install_binary

    local options=()
    while [ $# -gt 0 ]; do
        case "$1" in
            --archive) options+=(--archive); shift ;;
            --validator-names)
                [ -f "${2:-}" ] || die "--validator-names attend un fichier JSON existant"
                install -m 644 "$2" "$ETC/validator-names.json"
                options+=(--validator-names "$ETC/validator-names.json"); shift 2 ;;
            --node-name)
                # Sans espace : le nom est repris tel quel dans la ligne de commande du service.
                [[ "${2:-}" =~ ^[A-Za-z0-9-]{1,32}$ ]] || die "--node-name attend un nom de 1 à 32 lettres, chiffres ou tirets (par exemple Copper)"
                options+=(--node-name "$2"); shift 2 ;;
            --mempool-size)
                [[ "${2:-}" =~ ^[0-9]+$ ]] || die "--mempool-size attend un nombre de transactions"
                options+=(--mempool-size "$2"); shift 2 ;;
            --peer)
                [ $# -ge 2 ] || die "--peer attend une adresse"
                # Vérifiée avant de toucher au nœud : une adresse invalide
                # l'empêcherait de redémarrer.
                [[ "$2" =~ ^/(ip4|ip6|dns|dns4|dns6)/[^/]+/tcp/[0-9]+/p2p/[1-9A-HJ-NP-Za-km-z]+$ ]] ||
                    die "adresse de pair invalide : « $2 » (attendu : /ip4/IP/tcp/26656/p2p/12D3KooW…, affichée par « prepare » sur l'autre nœud)"
                options+=(--peer "$2"); shift 2 ;;
            --allow)
                [[ "${2:-}" =~ ^[1-9A-HJ-NP-Za-km-z]{40,60}$ ]] ||
                    die "identifiant invalide : « ${2:-} » (attendu : 12D3KooW…, affiché par « prepare --observer » sur le nœud public)"
                options+=(--allow "$2"); shift 2 ;;
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
    install -m 644 "$HERE/systemd/$unit" /etc/systemd/system/vinx-node.service
    systemctl daemon-reload
    systemctl enable vinx-node >/dev/null 2>&1
    systemctl restart vinx-node

    echo "Démarrage…"
    local status=""
    for _ in $(seq 1 30); do
        status="$(curl -fsS "$API/status" 2>/dev/null)" && break
        sleep 1
    done
    [ -n "$status" ] || die "le nœud ne répond pas ; voir : journalctl -u vinx-node -n 50"
    echo "$status"
    echo
    echo "Nœud démarré. Suivi : journalctl -u vinx-node -f"
}

# Hauteur finalisée du nœud local, vide s'il ne répond pas.
height() {
    curl -fsS --max-time 2 "$API/status" 2>/dev/null |
        grep -o '"height":[0-9]*' | cut -d: -f2
}

# Attend que le nœud finalise un bloc au-delà de la hauteur $1 (120 s au plus).
wait_progress() {
    local from="$1" h
    for _ in $(seq 1 120); do
        h="$(height)"
        if [ -n "$h" ] && [ "$h" -gt "$from" ]; then
            echo "hauteur $h : le nœud finalise des blocs"
            return 0
        fi
        sleep 1
    done
    return 1
}

upgrade() {
    local new="${1:-$HERE/vinx-node}"
    [ -x "$new" ] || die "nouveau binaire introuvable : $new"
    [ -x "$BIN" ] || die "aucun nœud installé : lance d'abord prepare puis start"
    local old_version new_version
    old_version="$("$BIN" --version | awk '{print $2}')"
    new_version="$("$new" --version | awk '{print $2}')" || die "$new ne s'exécute pas"
    if cmp -s "$new" "$BIN"; then
        echo "déjà en version $new_version"
        return 0
    fi
    echo "mise à jour $old_version -> $new_version"

    local before
    before="$(height)"
    [ -n "$before" ] || die "le nœud ne répond pas avant la mise à jour ; voir journalctl -u vinx-node"

    local stamp
    stamp="$(date +%Y%m%d-%H%M%S)"
    install -d -m 700 "$BACKUP"
    systemctl stop vinx-node
    install -m 755 "$BIN" "$BACKUP/vinx-node.previous"
    echo "sauvegarde des données dans $BACKUP/data-$stamp.tar.gz…"
    tar czf "$BACKUP/data-$stamp.tar.gz" -C "$(dirname "$DATA")" "$(basename "$DATA")"
    # Garde les trois dernières sauvegardes.
    ls -1t "$BACKUP"/data-*.tar.gz | tail -n +4 | xargs -r rm --

    install -m 755 "$new" "$BIN"
    systemctl start vinx-node
    if wait_progress "$before"; then
        echo "mise à jour terminée : $new_version"
        return 0
    fi

    echo "le nœud ne finalise pas de bloc : retour à $old_version" >&2
    rollback "$before"
    die "mise à jour annulée ; journal de la tentative : journalctl -u vinx-node --since '-5min'"
}

# Ne restaure que le binaire : les données gardent la mémoire des votes déjà
# signés, ce qui empêche le validateur de voter deux fois à la même hauteur.
# $1 : hauteur à dépasser (par défaut, la hauteur actuelle).
rollback() {
    [ -x "$BACKUP/vinx-node.previous" ] || die "aucun binaire précédent dans $BACKUP"
    local before="${1:-$(height)}"
    systemctl stop vinx-node
    install -m 755 "$BACKUP/vinx-node.previous" "$BIN"
    systemctl start vinx-node
    echo "binaire restauré : $("$BIN" --version)"
    wait_progress "${before:-0}" ||
        die "le nœud ne finalise toujours pas ; voir journalctl -u vinx-node -n 50"
}

reset_chain() {
    local genesis="${1:-}"
    [ -n "$genesis" ] && [ -f "$genesis" ] || die "usage : reset-chain nouvelle-genesis.json"
    [ -f "$ETC/vinx-node.env" ] || die "aucun nœud installé ici : utilise prepare puis start"
    grep -Eq '"chain_id": *"[^"]*(testnet|devnet)' "$genesis" ||
        die "reset-chain est réservé aux réseaux de test (chain_id contenant testnet ou devnet)"
    if cmp -s "$genesis" "$ETC/genesis.json"; then
        echo "cette genèse est déjà installée"
        return 0
    fi
    install_binary
    local stamp
    stamp="$(date +%Y%m%d-%H%M%S)"
    install -d -m 700 "$BACKUP"
    systemctl stop vinx-node
    echo "sauvegarde de l'ancienne chaîne dans $BACKUP/chaine-$stamp.tar.gz…"
    tar czf "$BACKUP/chaine-$stamp.tar.gz" -C "$(dirname "$DATA")" "$(basename "$DATA")" \
        -C "$ETC" genesis.json
    find "$DATA" -mindepth 1 -maxdepth 1 ! -name network.key -exec rm -rf -- {} +
    install -m 644 "$genesis" "$ETC/genesis.json"
    systemctl start vinx-node
    local status=""
    for _ in $(seq 1 30); do
        status="$(curl -fsS "$API/status" 2>/dev/null)" && break
        sleep 1
    done
    [ -n "$status" ] || die "le nœud ne répond pas ; voir : journalctl -u vinx-node -n 50"
    echo "$status"
    echo
    echo "Nœud passé sur la nouvelle genèse. Ancienne chaîne : $BACKUP/chaine-$stamp.tar.gz"
}

case "${1:-}" in
    prepare) shift; prepare "$@" ;;
    start) shift; start "$@" ;;
    upgrade) shift; upgrade "$@" ;;
    rollback) rollback ;;
    reset-chain) shift; reset_chain "$@" ;;
    *) die "usage : $0 prepare [--observer] | start genesis.json [--archive] [--peer ADRESSE]... [--allow ID]... | upgrade [binaire] | rollback | reset-chain genesis.json" ;;
esac
