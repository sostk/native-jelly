# earc_route: the HBR support Native Jelly installs on rooted TVs

A Linux kernel module, **GPL-2.0-only** (`LICENSE`, `NOTICE`); it is a separate program that Native
Jelly ships beside itself, never links, and loads only when the owner installs it from Settings.

## Why it exists

Native Jelly sends DTS-HD and TrueHD to a receiver by swapping the IEC 61937 bursts LG's own audio
library writes for a Dolby Digital Plus title (`rust-modules/src/player/es_carrier.rs`). LG runs that
route as a stereo 192 kHz link: enough for most DTS-HD MA, not for TrueHD (MAT) or full-rate
DTS-HD MA / DTS:X. While Native Jelly holds this module's lease (`/dev/earc_route`, written `2`),
the module's hooks make LG configure the same eARC link as 8 channels at 192 kHz (HBR):

| Hook (LG kernel function + offset) | Effect while the lease is held |
|---|---|
| `AUDIO_SetIecOutEncodedType` | encoded type `arc_type` (6, MAT/HBR) |
| `sndout_earc_output_type` | clears LG's mixing flag |
| `_AUDIO_SetSRCFormat` | keeps 192 kHz / 8 channels |
| `sndout_connect_output+0x94` | LG's bypass connect for the leased renderer |
| `_AUDIO_ConnectOutput_FindOutModule+0xc4` | ES output, `es_ch` channels |
| `_AUDIO_ConnectOutput_FindOutCtrlModule+0x22c` | `es_rate` instead of LG's 48 kHz default |

Without the lease the hooks pass through: nothing changes for LG's own apps.

## Coverage

`out/earc_route-4.4.ko` is built for webOS 6 kernels `4.4.84` (`vermagic` in `out/earc_route-4.4.json`)
and was tested on an **LG OLED C1, firmware 03.53.45**, with a Sony STR-DN1080 (DTS-HD MA, DTS:X,
TrueHD Atmos). `earc-route-load.sh` refuses any kernel whose stock modules disagree with the build,
and the module checks every instruction it hooks and refuses to arm on anything else, so other
models and firmware get no HBR (the stock 192 kHz route still works) rather than a wrong patch.

## Source and build

From [dts_restore_plus](https://github.com/josippapez/dts_restore_plus) `webos25/kmod` at
`e42c98eb6ac793bdccab565501384dadb0bd6720`, with `native-jelly.patch` applied (the hooks above,
their parameters, and the host test for them). Only the 4.4 (webOS 6) target is vendored.

Building needs three private files from your own TV (`kernel.bin`, `/proc/kallsyms` as
`kallsyms.txt`, and one stock module as `stock-module.ko`; they are checked against, never
embedded) and Docker:

```sh
sh build.sh /path/to/capture        # writes out/earc_route-4.4.ko and out/earc_route-4.4.json
sh test/legacy-audio/run.sh /path/to/capture   # QEMU host test, 1 and 4 CPUs
```

`ci/check-packaged-elf.py` accepts the module in the package only with the SHA-256 recorded in
`out/earc_route-4.4.json`.
