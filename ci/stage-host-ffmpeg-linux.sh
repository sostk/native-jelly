#!/bin/sh
# Stage the HOST FFmpeg for the Linux desktop simulator into pkg/linux-host/.
#
# The Linux twin of ci/stage-host-ffmpeg.sh, and simpler: ELF needs no install-name rewrite,
# because `ff.rs` opens the libraries in dependency order with RTLD_GLOBAL and the loader then
# resolves each DT_NEEDED SONAME against the copy already open. What it cannot do is sit beside the
# ARM libraries: those carry the SAME file names (`libavformat-plx.so.63`) in pkg/, so the x86_64
# build lives in a subdirectory that `ff.rs` lists as its own candidate. `APP_FILES` is an explicit
# list, so nothing here can reach an .ipk or a television.
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PREFIX="$ROOT/vendor/ffmpeg-prefix-host/lib"
OUT="$ROOT/pkg/linux-host"
mkdir -p "$OUT"
for name in libavutil-plx.so.61 libavcodec-plx.so.63 libavformat-plx.so.63 libswscale-plx.so.10; do
  [ -e "$PREFIX/$name" ] || { echo "no host $name in $PREFIX — run 'HOST=1 ci/build-ffmpeg.sh'" >&2; exit 1; }
  cp -L "$PREFIX/$name" "$OUT/$name.tmp.$$"
  mv "$OUT/$name.tmp.$$" "$OUT/$name"
done
