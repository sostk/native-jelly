# Credits, licence and trademarks

## PlxNative

Native Jelly is built on [PlxNative](https://github.com/GLinnik21/plx-native), the native Plex
client for LG webOS televisions created by **Gleb Linnik** ([GLinnik21](https://github.com/GLinnik21)).
Its native player, its GPU-drawn interface and much of its architecture and implementation are
PlxNative's work; Native Jelly adapted and extended them to sign in to and play from Jellyfin.
Thank you for making it available to build on.

For PlxNative's own history, documentation and development, see the
[original project](https://github.com/GLinnik21/plx-native). The PlxNative name, artwork and
branding remain its authors'; Native Jelly claims none of them.

## Contributors

Thanks to everyone who has contributed code, testing and reports. Contributions are credited in each
[release note](release-notes/) and in the commit history.

## Third-party components

Native Jelly contains or builds on third-party components (FFmpeg, libass, FreeType, HarfBuzz,
fonts and Rust crates, among others). Their licences and notices are in
[THIRD-PARTY-NOTICES.md](../THIRD-PARTY-NOTICES.md) and [licenses/](../licenses/). Please retain
them when redistributing Native Jelly.

The optional HBR support for rooted TVs uses the `earc_route` kernel module (GPL-2.0-only), from
[dts_restore_plus](https://github.com/josippapez/dts_restore_plus) with a webOS 6 patch by
[Mokto](https://github.com/Mokto). It is not part of the Native Jelly package; its complete source
is in [companion/hbr/earc_route/](../companion/hbr/earc_route/).

## Licence

Native Jelly is distributed under **GPL-3.0-or-later** ([LICENSE](../LICENSE)), subject to the
licensing terms of PlxNative and of its third-party components; [LICENSING.md](../LICENSING.md)
has the details. Every release attaches its complete corresponding source.

## Trademarks

Native Jelly is an independent community project. It is **not affiliated with, endorsed by, or
sponsored by** Jellyfin, LG Electronics, or the PlxNative project. "Jellyfin", "LG", "webOS" and
other trademarks belong to their owners and are used only to name the services, platforms and
technologies Native Jelly works with. See [TRADEMARKS.md](../TRADEMARKS.md).
