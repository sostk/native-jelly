# Security policy

## Reporting a vulnerability

Report privately, **not** as a public issue:

- **GitHub Security Advisories** — <https://github.com/sostk/native-jelly/security/advisories/new>
  (it is private, it threads, and it produces a CVE if one is warranted)

This is a one-person unpaid project, so the honest service level is: acknowledged within **7 days**,
an assessment within **30**. If you have not heard back in a week, open a public issue saying only
*"sent a security report on <date>, no reply"* — with no details.

Native Jelly is a fork of [PlxNative](https://github.com/GLinnik21/plx-native). A flaw that is also
present upstream is worth reporting there too, through that project's own security policy.

Please give me a reasonable window to ship a fix before disclosing. There is no bounty; I will credit
you in the release note unless you ask me not to.

## What is in scope

The app, its packaging, and the host-side tools in `tools/` and `ci/`. Concretely, the things worth
looking at:

- **The `/tmp` trigger surface.** `/tmp` is mode 1777 in webOS's production jail, so any co-resident
  process can create files there. Roughly forty `nativejelly-*` files change behaviour, and three are
  outright takeovers — `nativejelly-token` beats the signed-in session, `nativejelly-servers` injects a
  server and its token, `nativejelly-url` replaces the stream. **All of it is compiled out of a
  release build** by dropping the `devtriggers` cargo feature, and `ci/check-elf.sh` measures that
  on the shipped bytes rather than asserting it. A release binary that still carries any of it is a
  valid report, and a serious one.
- **The event log.** `nativejelly-events.log` is created 0600 and every line goes through
  `eventlog::scrub::scrub_local` before the write. A line that reaches it carrying a credential, a
  Jellyfin access token, a server's host name, a user name, a household name or anything about what
  is being watched is a valid report — see [PRIVACY.md](PRIVACY.md) for the contract that is meant
  to hold.
- **The Jellyfin sign-in file.** `jellyfin.json` holds the server address, the user, and the access
  token that server issued — never the password. It is written 0600 through the same atomic door as
  the session file, and is **not** encrypted with the Key Manager. A way to read it from another
  process, or to make the app write it somewhere world-readable, is in scope. So is any path where
  the password outlives the sign-in request.
- **TLS.** Certificate verification is on for every HTTPS request. When a server's certificate
  fails only its validity-date check (the television has no battery-backed clock) and a public key
  was remembered for that exact host and port from an earlier fully verified connection, the
  request is repeated with the chain-and-date check replaced by a pin on that key; the name check
  stays on. Telemetry and any host with no remembered key never take this path (`net.rs`,
  `net::keypin`). The product path talks only to the Jellyfin server whose address you enter.
  Over `http://`, your password, access token and everything Native Jelly exchanges with that
  server travel without encryption — enter `https://` for a server you reach over the internet.
  A token sent over plaintext to an origin the user did not enter, or to a server other than the
  one the token names, is in scope.
  Only an explicit developer-trigger build can otherwise allow the lab path, and it logs the
  exception without the URL.
  Anything that disables, downgrades or bypasses these rules is in scope — including a key-mode
  request that accepts a different key or a wrong name; so is any path where a failure to *set* a
  security option results in a request going out anyway.
- **The session file.** `<id>-auth.json` holds one access token per server your account can reach.
  It is encrypted with the firmware's authenticated Key Manager where
  `com.webos.service.keymanager3` is available and permitted, with a 0600 plaintext compatibility
  fallback otherwise. The legacy `com.palm.keymanager` AES-CFB interface is not used because it
  provides no authenticated-encryption operation. The file is always created 0600 through
  `open(2)`'s own mode argument. A downgrade of an existing encrypted file, a way to read it from
  another process, or a way to make the app write it somewhere world-readable is in scope.
- **The bundled FFmpeg.** Built from unmodified FFmpeg 9.0 with demuxers, parsers and subtitle
  decoders only — it is fed untrusted bytes from the network, so parser bugs reachable through
  `ff.rs` are in scope. Report FFmpeg's own bugs upstream as well.

## What is not in scope

- Post-compromise access by an attacker who already has root on the television. Root is not an app
  prerequisite; a report whose only precondition is an already-rooted OS describes a platform
  compromise rather than an app sandbox escape.
- The webosbrew Homebrew Channel, webOS itself, LG's own libraries, or the Jellyfin server.
  Report those to their maintainers.
- Missing hardening that costs nothing to an attacker who is already executing code in the app's
  jail, unless you can show a concrete consequence.

## What this app does not have

No account of its own, no server, no payment path, and no user-generated content. It signs in to
**your** Jellyfin server and talks only to it.

**It can have telemetry.** A binary built with `NJ_SENTRY_DSN` / `NJ_POSTHOG_KEY` set carries a
Sentry DSN and a PostHog project key — both **write-only ingest credentials**, publishable by
design, which permit sending to a project and grant no read of anything in it. A build without them
sends no reports at all. First run asks about crash reports
and product analytics separately. The first answer remains a draft; answering the second records
both choices, and only a **Share** answer enables that category and permits its POSTs to
`ingest.de.sentry.io` or `eu.i.posthog.com`. `BACK` navigates without recording a refusal. Later
changes live under Account → Settings → Privacy & data, where **Done** commits and `BACK` discards.
The Sentry
**auth token** is the real secret in this system: it can read and delete the project, and it must
never enter the binary — keep it only as a CI secret for `sentry-cli`.

In scope for a report, and worth naming since a "no telemetry endpoint" line told researchers not to
look here: the consent gate failing open, an identifier existing before product analytics is
explicitly enabled or surviving its withdrawal, anything that gets a runtime string past
`diag::schema`'s no-owned-strings guarantee,
and the spool's file mode or its contents. [PRIVACY.md](PRIVACY.md) is the full account of what
leaves the television.
