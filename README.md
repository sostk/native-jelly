# Native Jelly

A fast, unofficial **Jellyfin client for LG webOS TVs**. Its interface is drawn natively on the TV's
GPU, not in a web page, and video goes straight to the TV's own hardware decoder.

Native Jelly is built on [PlxNative](https://github.com/GLinnik21/plx-native), the native Plex client
by Gleb Linnik ([GLinnik21](https://github.com/GLinnik21)), and adapted for Jellyfin. Its player,
interface and architecture are his work; see [credits](docs/credits.md).

![Native Jelly's home screen](docs/screenshots/home.jpg)

[Website](https://sostk.github.io/native-jelly/) · [Install](docs/install-and-verify.md) ·
[Latest release](https://github.com/sostk/native-jelly/releases/latest) ·
[Troubleshooting](docs/troubleshooting.md) · [Support on Ko-fi](https://ko-fi.com/swastikpoudel)

## Features

- Sign in to your Jellyfin server; several users on one TV, each with their own picture
- Home, Continue Watching, your libraries (movies, shows, mixed and home videos) and Search
- Plays files directly when the TV can decode them; your server converts the rest
- Audio the TV cannot decode (TrueHD, FLAC, LPCM) converted to Dolby Digital on the TV
- DTS and Dolby TrueHD sent unchanged to an AV receiver or soundbar —
  [lossless audio](docs/lossless-audio.md)
- Dolby Vision, HDR10, audio and subtitle track choice, styled subtitles, scrub previews, Up Next
- English, Spanish, Belarusian and Russian

## Install

No root is needed. Install the `.ipk` from the
[latest release](https://github.com/sostk/native-jelly/releases/latest) with the
[Homebrew Channel](https://github.com/webosbrew/webos-homebrew-channel) or with Developer Mode and
[dev-manager-desktop](https://github.com/webosbrew/dev-manager-desktop). Step by step, including how
to check the download: [docs/install-and-verify.md](docs/install-and-verify.md).

## Will it work on my TV?

Native Jelly starts on **webOS 4.0 and newer**. Video playback is verified on an LG C1 (2021,
webOS 6) and reported by users on other 2021 sets; on other models it is only known to start. If you
try it on another TV, please [tell us what happened](https://github.com/sostk/native-jelly/issues),
whether it worked or not. Current limits are listed in each
[release note](https://github.com/sostk/native-jelly/releases/latest).

## Documentation

| | |
|---|---|
| [Install and verify](docs/install-and-verify.md) | Installing, updating, checking the download, Developer Mode |
| [Troubleshooting](docs/troubleshooting.md) | Common problems, and what to include in a report |
| [Lossless audio](docs/lossless-audio.md) | DTS and TrueHD to a receiver, and HBR on rooted TVs |
| [Privacy](PRIVACY.md) | What the app sends, and to whom (crash reports and analytics are opt-in) |
| [Building](docs/building.md) | Development environment, build and tests |
| [Contributing](CONTRIBUTING.md) · [Security](SECURITY.md) | How to contribute, and how to report a vulnerability |
| [Release notes](docs/release-notes/) | What changed in each version |
| [Credits](docs/credits.md) | PlxNative, third-party components, licence and trademarks |

## Support

Native Jelly is free and open source. If it is useful to you, you can
[support it on Ko-fi](https://ko-fi.com/swastikpoudel).

## Credits and licence

Native Jelly is built on [PlxNative](https://github.com/GLinnik21/plx-native) by Gleb Linnik, adapted
for Jellyfin. It is licensed under [GPL-3.0-or-later](LICENSE); third-party notices are in
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md), and the full credits in
[docs/credits.md](docs/credits.md).

Native Jelly is not affiliated with, endorsed by, or sponsored by Jellyfin, LG Electronics, or the
PlxNative project.
