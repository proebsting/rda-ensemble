# Sending this to someone

```sh
./dist/build-macos.sh
```

builds everything into `dist/out`, for Intel and Apple Silicon both:

| | |
|---|---|
| `rda-ensemble.app` | the window |
| `rda-ensemble.dmg` | the same thing, as a disk image to send |
| `rda-ensemble` | the command line tool |
| `window.md`, `command-line.md` | the documentation |

Built and measured: the app is 21 MB, the disk image 8.7 MB, the CLI 9.7 MB.
Both binaries are universal, and both link nothing but `libSystem`, `libobjc`
and `libiconv` — all part of macOS. There is nothing to install alongside
them.

## The one real obstacle: macOS will refuse to open it

Anything downloaded from the internet is quarantined. An **unsigned**
application that is quarantined gets

> "rda-ensemble" cannot be opened because the developer cannot be verified.

and on macOS 15 and later the old right-click → Open trick no longer works.
The recipient has to go to **System Settings → Privacy & Security**, find the
message near the bottom, and press **Open Anyway**. It works, and it is one
sentence to explain, but it asks a law professor to override a security
warning, which is a thing to ask.

The command line tool has the same problem with an easier fix, because its
users already have a terminal:

```sh
xattr -d com.apple.quarantine rda-ensemble
```

### Making it just open

You need an **Apple Developer Program** membership, \$99 a year. With it:

1. Create a *Developer ID Application* certificate in the developer portal and
   install it in your keychain.
2. Store an app-specific password once:
   ```sh
   xcrun notarytool store-credentials rda \
     --apple-id you@example.com --team-id TEAMID --password APP-SPECIFIC-PASSWORD
   ```
3. Build with both set:
   ```sh
   SIGN_ID="Developer ID Application: Your Name (TEAMID)" \
   NOTARY_PROFILE=rda ./dist/build-macos.sh
   ```

The script signs with the hardened runtime, submits the disk image to Apple,
waits, and staples the ticket so it opens even offline. After that the app
opens by double-clicking, with no warning and nothing to explain.

There is no way around this short of the membership. Self-signed certificates
do not help: Gatekeeper checks for an Apple-issued Developer ID, not for a
signature as such.

## Other platforms

Only macOS is built here, and cross-compiling is not the answer: the window
needs each platform's own webview, and the compression needs a C compiler for
that platform.

- **Windows** — build on Windows. `cargo tauri build` produces an `.msi` and
  an `.exe` installer. SmartScreen will warn about an unsigned binary in much
  the same way, and an EV code-signing certificate costs considerably more
  than Apple's.
- **Linux** — build on Linux. `cargo tauri build` produces `.deb`, `.rpm` and
  an AppImage. Nothing objects to an unsigned binary, so this is the easy one.

The usual answer to all of this is GitHub Actions: three runners, one release.
That is worth setting up when there is a second person to send it to, and not
before.

## What the recipient actually needs

Nothing. No Rust, no Python, no DRA account. The app downloads the state data
it needs on first use and caches it.

Two things worth saying when you send it:

- **It downloads data the first time.** Michigan is about 4 MB, Illinois about
  8 MB. The window says what it is fetching before it does.
- **Where results go is theirs to choose.** The output directory is not
  remembered between people, deliberately: a settings file carries the
  decisions that define an ensemble and none of the paths that would make it
  yours rather than theirs.

## Reproducing someone else's run

This is the part worth pointing out, because it is what the tool is for. Every
run writes `settings.json` beside its results, holding every decision and no
paths. Given that file, from the window: **Load settings**, then **Run**. From
a terminal:

```sh
rda-ensemble replay --settings their-settings.json --out results
```

Either reproduces the ensemble exactly. A `manifest.json` from someone's
output directory works the same way, which matters because that is what people
actually have when they have not thought to save settings.
