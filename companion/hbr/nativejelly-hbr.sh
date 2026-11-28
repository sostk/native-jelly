#!/bin/sh
# nativejelly-hbr.sh: HBR audio support for Native Jelly on a rooted LG TV (vendor/earc_route).
#
#   install APPDIR   copy the module, its loader and this script to /var/lib/webosbrew/nativejelly-hbr,
#                    hook this script into Homebrew Channel's boot scripts, then run `boot`
#   boot             load the module (dts_restore_plus's loader checks the kernel first), set its
#                    HBR parameters, and put its lease node in every Native Jelly sandbox
#   remove           undo `install`; the module stays loaded, switched off, until the next reboot
#   status           one line: installed / active
#
# Runs as root (Homebrew Channel's exec service, or its boot hooks). Prints one result line.
D=/var/lib/webosbrew/nativejelly-hbr
HOOK=/var/lib/webosbrew/init.d/nativejelly-hbr
P=/sys/module/earc_route/parameters

nodes() {
    minor=$(cut -d: -f2 /sys/class/misc/earc_route/dev 2>/dev/null) || return 1
    for jail in /var/palm/jail/com.sostk.nativejelly*; do
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
remove)
    rm -f "$HOOK"; rm -rf "$D"
    rm -f /var/palm/jail/com.sostk.nativejelly*/dev/earc_route
    [ -w "$P/enabled" ] && echo 0 > "$P/enabled"
    echo "hbr: removed"
    ;;
status)
    if [ -x "$HOOK" ] && [ "$(cat "$P/enabled" 2>/dev/null)" = 1 ]; then echo "hbr: active"
    elif [ -x "$HOOK" ]; then echo "hbr: installed, not active (reboot)"
    else echo "hbr: not installed"; fi
    ;;
*)
    echo "usage: $0 install APPDIR | boot | remove | status"; exit 2
    ;;
esac
