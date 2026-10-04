# Native Jelly

A fast, unofficial **Jellyfin client for LG webOS televisions**.

**Native, not a web page** — Native Jelly is designed to provide a smooth, responsive TV experience using the television's native graphics and video capabilities.

Native Jelly is based on [PLX Native](https://github.com/GLinnik21/plx-native), originally created by **GLinnik21**. The project has been substantially adapted and extended to work with **Jellyfin**.

> **Native Jelly is an independent project and is not affiliated with or endorsed by Jellyfin, LG Electronics, or the original PLX Native project.**

---

## Why Native Jelly?

Jellyfin provides an excellent open-source media server, but the quality of the client experience can vary considerably between platforms.

Native Jelly aims to provide a **fast, modern and TV-focused Jellyfin experience** on LG webOS televisions.

The project builds on the native architecture of PLX Native rather than relying on a traditional browser-based application.

The goal is simple:

* Fast navigation
* Smooth remote control interaction
* Native-feeling TV interface
* Efficient video playback
* Support for Jellyfin servers
* Designed specifically for the 10-foot TV experience

---

## What it looks like

Native Jelly is designed around a modern streaming-service style interface, with large artwork, clear focus states and layouts designed for navigation using an LG Magic Remote or directional remote.

### Home

The home screen provides quick access to your Jellyfin content and recently watched media.

### Libraries

Browse your Jellyfin libraries using a TV-friendly grid and navigation system.

### Search

Search your Jellyfin library and quickly find movies, TV shows and other available media.

### Details

View artwork, metadata, descriptions and available playback options before starting a movie or episode.

### Player

The player provides a TV-focused playback experience with playback controls and support for available audio and subtitle tracks.

> Screenshots will be added as Native Jelly develops.

---

## Features

* **Native TV-focused interface**
* **Jellyfin server support**
* Browse Jellyfin libraries
* Movie and TV show browsing
* Search
* Continue watching
* Media details
* Playback controls
* Audio track selection
* Subtitle selection
* Resume playback
* Remote-friendly navigation
* Designed for LG webOS televisions
* Native video playback where supported by the television
* Support for server-side transcoding where required
* Designed for 10-foot viewing distances

Additional Jellyfin functionality will continue to be added as development progresses.

---

## LG webOS Support

Native Jelly is intended for **LG webOS televisions**.

The project is based on the native architecture of PLX Native, which targets older LG webOS televisions as well as newer devices.

### Minimum webOS version

**webOS 4.0 or newer** is currently targeted.

Compatibility can vary between television models, webOS versions and hardware generations.

If you test Native Jelly on a television that is not currently covered by the project, feedback and testing reports are very welcome.

---

## Developer Mode

**Root access is not required.**

Native Jelly can be installed on a standard LG webOS television using **LG Developer Mode**.

Developer Mode allows applications to be installed outside the LG Content Store.

Keep in mind that LG Developer Mode installations require periodic renewal. If Developer Mode expires, applications installed through it may be removed.

For installation instructions, see:

* [Installation Guide](docs/install-and-verify.md)
* [Troubleshooting](docs/troubleshooting.md)

---

## Installing

The recommended installation process is:

1. Enable **Developer Mode** on your LG webOS television.
2. Connect the television and computer to the same network.
3. Install **webOS Dev Manager**.
4. Build or download the Native Jelly `.ipk`.
5. Install the package on your television.
6. Launch Native Jelly from the LG launcher.

Detailed installation and verification instructions are available in:

**[docs/install-and-verify.md](docs/install-and-verify.md)**

### Homebrew Channel

If Native Jelly is made available through Homebrew Channel, it can also be installed and updated through the Homebrew ecosystem.

---

## Building

Native Jelly is based on the PLX Native codebase and retains its native build architecture.

For development and build instructions, see:

**[docs/building.md](docs/building.md)**

The build documentation contains the required development environment, build process and testing workflow.

---

## Known Issues

Native Jelly is actively being developed and tested.

Compatibility can vary depending on:

* LG TV model
* webOS version
* TV chipset
* Video codec
* Audio codec
* Subtitle format
* Jellyfin server configuration
* Direct Play vs transcoding

If you encounter a problem, please check:

**[Troubleshooting](docs/troubleshooting.md)**

If the issue is not covered, please open an issue and include:

* LG TV model
* webOS version
* Native Jelly version
* Jellyfin server version
* Playback mode
* Media codec information
* Relevant logs

Testing on additional LG television models is especially helpful.

---

## Privacy

Native Jelly is designed to communicate with **your Jellyfin server**.

Your media library is not sent to the Native Jelly project.

Native Jelly does not require a third-party media catalogue to browse your Jellyfin library.

For the complete privacy statement, see:

**[PRIVACY.md](PRIVACY.md)**

---

## Scope

Native Jelly is primarily focused on providing a **Jellyfin client for LG webOS televisions**.

The project is intentionally focused on the TV experience rather than attempting to reproduce every feature available in every Jellyfin client.

The focus is:

* Movies
* TV shows
* Library browsing
* Search
* Playback
* Subtitles
* Audio tracks
* Continue watching
* A responsive TV interface

More functionality may be added as the project develops.

---

## Contributing

Issues and pull requests are welcome.

If you have an LG television that behaves differently from the development hardware, please consider reporting it.

Useful information includes:

* TV model
* webOS version
* Jellyfin server version
* Media information
* Steps to reproduce the problem
* Logs

For development and build information, see:

**[docs/building.md](docs/building.md)**

Security issues should be reported through:

**[SECURITY.md](SECURITY.md)**

---

## Credits & Attribution

Native Jelly would not exist without the work that went into **PLX Native**.

### PLX Native

Original project:

**https://github.com/GLinnik21/plx-native**

Original author:

**GLinnik21 / Gleb Linnik**

Native Jelly is based on the PLX Native codebase and retains portions of its original architecture and implementation.

We would like to give full credit to **GLinnik21** for creating PLX Native and for the technical foundation on which Native Jelly is built.

Thank you for making the original project available to build upon.

---

## Original Project

You can find the original PLX Native project here:

**[PLX Native](https://github.com/GLinnik21/plx-native)**

Please refer to the original project for its history, original documentation and upstream development.

---

## Third-Party Components

Native Jelly contains or builds upon third-party components.

Their respective licences and notices can be found in:

* `THIRD-PARTY-NOTICES.md`
* `licenses/`

Please retain these notices when redistributing Native Jelly.

---

## Licence

Native Jelly is distributed under the **GPL-3.0-or-later** licence, subject to the applicable licensing terms of the original PLX Native project and its third-party components.

See:

**[LICENSE](LICENSE)**

The original PLX Native project and its associated branding remain the work of their respective authors.

Native Jelly does not claim ownership of the original PLX Native name, artwork or branding.

---

## Trademarks & Disclaimer

Native Jelly is an independent community project.

**Native Jelly is not affiliated with, endorsed by, or sponsored by Jellyfin, LG Electronics, or the original PLX Native project.**

"Jellyfin", "LG", "webOS" and other trademarks belong to their respective owners.

Native Jelly uses these names only to identify the services, platforms and technologies with which the software is intended to work.
