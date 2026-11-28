#!/bin/sh
# build.sh: package the Native Jelly HBR companion app as dist/com.sostk.nativejelly.hbr_<version>_all.ipk.
# Needs ares-package (the webOS CLI). The module is the prebuilt earc_route/out/earc_route-4.4.ko,
# accepted only with the SHA-256 recorded beside it; earc_route/README.md says how to rebuild it.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
dist="$here/dist"
stage="$dist/stage"
ko="$here/earc_route/out/earc_route-4.4.ko"

want=$(sed -n 's/.*"moduleSha256": *"\([0-9a-f]*\)".*/\1/p' "$here/earc_route/out/earc_route-4.4.json")
got=$(sha256sum "$ko" | cut -d' ' -f1)
[ -n "$want" ] && [ "$want" = "$got" ] || { echo "build.sh: $ko does not match earc_route-4.4.json" >&2; exit 1; }

rm -rf "$stage" && mkdir -p "$stage/hbr" "$stage/licenses"
cp "$here"/app/* "$stage/"
cp "$ko" "$here/earc_route/earc-route-load.sh" "$here/nativejelly-hbr.sh" "$stage/hbr/"
# The module's complete corresponding source travels with it (GPL-2.0-only).
mkdir -p "$stage/source"
(cd "$here" && tar -cf - --exclude='out/*.ko' earc_route) | tar -xf - -C "$stage/source"
cp "$here/earc_route/LICENSE" "$stage/licenses/GPL-2.0.txt"
cp "$here/../../LICENSE" "$stage/licenses/GPL-3.0.txt"
ares-package -o "$dist" "$stage"
rm -rf "$stage"
ls "$dist"/*.ipk
