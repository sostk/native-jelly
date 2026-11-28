#!/bin/sh
# nativejelly-hbr.sh: HBR audio support for Native Jelly on a rooted LG TV (earc_route/).
#
#   install APPDIR   copy the module, its loader and this script to /var/lib/webosbrew/nativejelly-hbr,
#                    hook this script into Homebrew Channel's boot scripts, then run `boot`
#   boot             load the module (dts_restore_plus's loader checks the kernel first), set its
#                    HBR parameters, and put its lease node in each Native Jelly sandbox
#   refresh          put the lease node in each Native Jelly sandbox again (after installing or
#                    first opening Native Jelly)
#   remove           undo `install`; the module stays loaded, switched off, until the next reboot
#   status           key=value lines for the companion app's status panel
#
# Runs as root (Homebrew Channel's exec service, or its boot hooks). install, boot, refresh and
# remove end with one result line: `hbr: active`, `hbr: removed` or `hbr: REFUSED: <why>`.
D=/var/lib/webosbrew/nativejelly-hbr
HOOK=/var/lib/webosbrew/init.d/nativejelly-hbr
P=/sys/module/earc_route/parameters
APPS="com.sostk.nativejelly com.sostk.nativejelly.debug"

# The node in every Native Jelly sandbox that exists. A sandbox appears the first time its app runs.
nodes() {
    minor=$(cut -d: -f2 /sys/class/misc/earc_route/dev 2>/dev/null) || return 1
    [ -n "$minor" ] || return 1
    for app in $APPS; do
        jail=/var/palm/jail/$app
        [ -d "$jail/dev" ] || continue
        rm -f "$jail/dev/earc_route"
        mknod "$jail/dev/earc_route" c 10 "$minor" && chmod 666 "$jail/dev/earc_route"
    done
}

case "$1" in
install)
    [ "$(id -u)" = 0 ] || { echo "hbr: REFUSED: not root (needs a rooted TV with Homebrew Channel)"; exit 1; }
    [ -f "$2/hbr/earc_route-4.4.ko" ] || { echo "hbr: REFUSED: no module in $2/hbr"; exit 1; }
    mkdir -p "$D" /var/lib/webosbrew/init.d || exit 1
    cp "$2/hbr/earc_route-4.4.ko" "$2/hbr/earc-route-load.sh" "$2/hbr/nativejelly-hbr.sh" "$D/" || exit 1
    printf '#!/bin/sh\nexec sh %s/nativejelly-hbr.sh boot\n' "$D" > "$HOOK" && chmod 755 "$HOOK" || exit 1
    exec sh "$D/nativejelly-hbr.sh" boot
    ;;
boot)
    if [ ! -e /sys/module/earc_route ]; then
        out=$(sh "$D/earc-route-load.sh" --dts-hd </dev/null 2>&1) || { echo "hbr: REFUSED: $(echo "$out" | tail -n 1)"; exit 1; }
    fi
    [ -w "$P/es_ch" ] || { echo "hbr: REFUSED: another earc_route build is loaded (dts_restore_plus?); reboot without it"; exit 1; }
    echo 8 > "$P/es_ch"; echo 192000 > "$P/es_rate"; echo 1 > "$P/es_out"; echo 6 > "$P/arc_type"
    echo 1 > "$P/enabled"
    nodes || { echo "hbr: REFUSED: no /dev/earc_route"; exit 1; }
    echo "hbr: active"
    ;;
refresh)
    [ "$(cat "$P/enabled" 2>/dev/null)" = 1 ] || { echo "hbr: REFUSED: not active"; exit 1; }
    nodes || { echo "hbr: REFUSED: no /dev/earc_route"; exit 1; }
    echo "hbr: active"
    ;;
remove)
    rm -f "$HOOK"; rm -rf "$D"
    for app in $APPS; do rm -f "/var/palm/jail/$app/dev/earc_route"; done
    [ -w "$P/enabled" ] && echo 0 > "$P/enabled"
    echo "hbr: removed"
    ;;
status)
    echo "root=$([ "$(id -u)" = 0 ] && echo 1 || echo 0)"
    echo "kernel=$(uname -r)"
    echo "installed=$([ -x "$HOOK" ] && echo 1 || echo 0)"
    echo "loaded=$([ -e /sys/module/earc_route ] && echo 1 || echo 0)"
    echo "ours=$([ -w "$P/es_ch" ] && echo 1 || echo 0)"
    echo "enabled=$(cat "$P/enabled" 2>/dev/null || echo 0)"
    for app in $APPS; do
        jail=/var/palm/jail/$app
        if [ -c "$jail/dev/earc_route" ]; then s=node
        elif [ -d "$jail/dev" ]; then s=no-node
        elif [ -d "/media/developer/apps/usr/palm/applications/$app" ] || [ -d "/media/cryptofs/apps/usr/palm/applications/$app" ]; then s=not-opened
        else s=not-installed; fi
        echo "app.$app=$s"
    done
    ;;
*)
    echo "usage: $0 install APPDIR | boot | refresh | remove | status"; exit 2
    ;;
esac
