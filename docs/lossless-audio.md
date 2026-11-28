# Lossless audio to a receiver

Many LG televisions cannot decode DTS, and none plays Dolby TrueHD from an app. With an AV receiver
or soundbar, Native Jelly can send these soundtracks to it unchanged, and the receiver decodes them.

**Turn it on**

1. Connect the receiver by HDMI eARC/ARC or optical, and make it the TV's sound output
   (*Settings → Sound → Sound Out*).
2. Set *Settings → Sound → Advanced Settings → Digital Sound Output* to **Pass Through**.
3. In Native Jelly, open *Settings → Video & playback → Lossless audio to receiver* and choose **On**.

The setting is used only while the sound goes to a receiver, and the output is checked again at the
start of every playback: on the TV's own speakers these bitstreams would be loud noise, so there the
TV's ordinary audio path is used.

**What reaches the receiver**

| Soundtrack | Without HBR | With HBR (rooted TV, see below) |
|---|---|---|
| DTS | DTS 5.1 core | DTS |
| DTS-HD MA | full DTS-HD MA up to about 6 Mb/s on webOS 6 over HDMI; otherwise its DTS 5.1 core | full rate |
| DTS:X | its DTS-HD MA or DTS core layers, as above | DTS:X |
| Dolby TrueHD / Atmos | converted to Dolby Digital on the TV | TrueHD and Atmos |

**HBR on rooted TVs.** Full-rate DTS-HD MA, DTS:X and TrueHD/Atmos need the TV's HDMI eARC link in
its 8-channel high-bitrate mode, which LG does not open to apps. On a rooted TV the `earc_route`
kernel module from [dts_restore_plus](https://github.com/josippapez/dts_restore_plus) can. Native
Jelly does not install kernel modules itself: it uses HBR automatically whenever a package on the TV
provides the module's `/dev/earc_route` lease node in Native Jelly's sandbox, and behaves exactly as
without HBR when none does.

On a rooted webOS 6 TV (kernel 4.4, Homebrew Channel) there are two ways to install it, both opt-in:

- **From Native Jelly.** With *Lossless audio to receiver* on, *Settings → Video & playback* shows
  *Atmos, TrueHD and DTS:X (root)*. Choose **Download and install** and confirm: Native Jelly
  downloads the module from this repository at a pinned commit, refuses it unless its SHA-256 matches
  the one built into the app, and installs it as root through the Homebrew Channel, loaded now and at
  every start of the TV. **Remove** undoes it.
- **With Native Jelly HBR**, a small companion app in [`companion/hbr/`](../companion/hbr/) that does
  the same in five guided steps.

The module runs in the TV's kernel, so use it at your own risk. Support for webOS 6 in
dts_restore_plus's **DTS Enabler** app is being proposed upstream; once it is there, that app
replaces the companion app. Tested on an LG C1 (webOS 6) with a Sony STR-DN1080 and with an LG
soundbar on eARC.

How each route works, for developers: [playback-settings.md](playback-settings.md).
