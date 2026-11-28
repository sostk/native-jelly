# Native Jelly HBR

A small companion app for **rooted LG webOS 6 TVs** that lets [Native Jelly](../../README.md) send
Dolby TrueHD and Atmos, DTS:X and full-rate DTS-HD MA to an AV receiver or soundbar over HDMI eARC.

LG runs an app's audio pass-through as a stereo 192 kHz link. These formats need the link's
8-channel high-bitrate (HBR) mode, which LG never opens to apps. This app installs the `earc_route`
kernel module, which can. While Native Jelly plays a lossless track it holds the module's lease
(`/dev/earc_route`) and the module makes LG set up the HBR link; at every other moment the TV's
sound works exactly as before. Without this app, Native Jelly still sends DTS and most DTS-HD MA, and
plays TrueHD as Dolby Digital.

This is a stopgap. Support for webOS 6 is being proposed upstream to
[dts_restore_plus](https://github.com/josippapez/dts_restore_plus)'s DTS Enabler app, where the module
comes from. Once it is there, use that instead.

## Requirements

- An LG TV on **webOS 6** (kernel 4.4), **rooted**, with the **Homebrew Channel**. Built and tested on
  an LG OLED C1 (kernel `4.4.84`, firmware 03.53.45), with a Sony STR-DN1080 and with an LG soundbar
  on eARC. The loader compares the kernel with the one the module was built for and refuses any
  other; the module checks every instruction it changes and refuses to arm on anything unexpected.
  On another TV the app says so and changes nothing.
- Native Jelly, installed and opened once (opening it creates the sandbox the lease node goes into).
- Not together with dts_restore_plus's own `earc_route` module. If that one is loaded, remove it there
  and restart the TV first.

**The module runs in the TV's kernel. Use it at your own risk.**

## Install and use

1. Build the package (below), then install it like any other homebrew app, for example
   `ares-install com.sostk.nativejelly.hbr_0.1.0_all.ipk`.
2. Open **Native Jelly HBR** and follow its five steps:
   1. **Root**: checked for you.
   2. **Native Jelly**: open it once if the app asks you to, then press *Check again*.
   3. **Turn on HBR**: loads the module now and adds it to the Homebrew Channel's startup scripts, so
      it comes back after every restart.
   4. **Sound output**: connect the receiver by HDMI eARC and choose it in *Settings › Sound › Sound
      Out*. Set *Advanced Settings › Digital Sound Output* to **Pass Through**.
   5. **Native Jelly setting**: *Settings › Video & playback › Lossless audio to receiver* › **On**.

Use the remote's left and right buttons to move between steps, and down to reach a step's buttons.

If you reinstall Native Jelly later, open it once and press *Fix it* in step 3.
**Turn off and remove** (step 3) deletes everything the app installed. The module is switched off at
once and unloaded at the next restart.

## How it works

The app is a plain web app with no root of its own. Every action runs
[`nativejelly-hbr.sh`](nativejelly-hbr.sh) as root through the Homebrew Channel's `exec` service:

| Action | What it does |
|---|---|
| `install APPDIR` | copies the module, its loader and the script to `/var/lib/webosbrew/nativejelly-hbr`, adds `/var/lib/webosbrew/init.d/nativejelly-hbr`, then runs `boot` |
| `boot` | loads the module (the loader checks the kernel first), sets its HBR parameters, and creates `/dev/earc_route` in the sandboxes of `com.sostk.nativejelly` and `com.sostk.nativejelly.debug` |
| `refresh` | creates the sandbox nodes again |
| `remove` | deletes the hook, the files and the nodes, and switches the module off |
| `status` | `key=value` lines the app's steps are drawn from |

## Build

```sh
sh companion/hbr/build.sh    # writes companion/hbr/dist/com.sostk.nativejelly.hbr_<version>_all.ipk
```

Needs `ares-package`. The script packages the prebuilt `earc_route/out/earc_route-4.4.ko` only if its
SHA-256 matches `earc_route/out/earc_route-4.4.json`, and puts the module's complete source and both
licences in the package. To rebuild the module itself, see [`earc_route/README.md`](earc_route/README.md).

Opened in a desktop browser, `app/index.html` previews its states without a TV: add `#fresh`, `#on`,
`#notopened`, `#noroot` or `#conflict` to the address.

## Licences and credits

- `earc_route/`: **GPL-2.0-only** (`earc_route/LICENSE`, `earc_route/NOTICE`). From
  dts_restore_plus `webos25/kmod`, derived from Glasshouse's native-route and lg-g5-dts, with the
  webOS 6 patch Théo Mathieu (Mokto) wrote for Native Jelly (`earc_route/native-jelly.patch`).
- The app and `nativejelly-hbr.sh`: GPL-3.0-or-later, like the rest of this repository.

Not affiliated with, endorsed by or supported by LG Electronics.
