# Desktop App Packaging

The web interface ships as a **single product file** on the desktop: the
`oxo-flow` binary already contains the CLI, the workflow engine, the web
server, and the SPA frontend. The bundle wraps it into the native app
formats below — no browser install surprises, no separate server process.

## How it works

```
oxo-flow.app (macOS) / oxo-flow.deb (Linux)
└── oxo-flow                # launcher script — macOS double-click runs
    │                       # the bundle with no arguments, so it cds to
    │                       # $HOME and execs:
    └── oxo-flow-bin serve --open   # the CLI: starts the local web server
                                    # and opens the default browser
    └── Resources/static/       # SPA assets (resolved relative to
                                # the executable at runtime)
    └── Resources/oxo-flow.icns # app icon (macOS)
```

macOS launches a `.app` bundle's executable with **no arguments**, so the
bundle contains a tiny launcher script (`Contents/MacOS/oxo-flow`) that
starts in `$HOME` and routes the launch to the CLI's `serve --open` entry
point (`Contents/MacOS/oxo-flow-bin`). This is what makes the desktop app
open the interface when double-clicked — Finder launches apps with
`cwd=/`, where the web server cannot create its SQLite database.

`--open` opens the interface in the system browser. Rendering stays in the
browser engine: the DAG canvas (React Flow) and CodeMirror editor are
mature browser technologies, and the browser gives users their own
extensions, password managers, and devtools. A native-webview shell
(Tauri) is a possible future enhancement for tray/notification integration,
not a prerequisite for the product experience.

## Prerequisites

1. Build the frontend first — a prebuilt SPA ships in
   `crates/oxo-flow-web/static/`, but rebuild it to bundle the latest UI:

   ```bash
   cd frontend && npm install && npm run build && cd ..
   ```

2. Install cargo-bundle once:

   ```bash
   cargo install cargo-bundle --locked
   ```

## macOS (.app + .dmg)

The GitHub release ships a hand-rolled `.app` + `.dmg` built by CI
(icon, launcher, and ad-hoc code signature included). To build a plain
cargo-bundle app locally (no launcher/icon/signature):

```bash
make bundle-macos
# → target/release/bundle/macos/oxo-flow.app
# → target/release/bundle/macos/oxo-flow.dmg (double-click to install)
```

The `.app` is self-contained: drag it to /Applications, double-click, and
the interface opens in your browser. Data (SQLite + workspace) lives in
the directory the app launches from — the launcher starts in `$HOME`
(Finder launches apps with `cwd=/`, where the server cannot create its
`oxo-flow.db`), so the desktop app keeps its data in your home directory.

### Install and first run (macOS)

1. **Do not double-click the app inside the DMG.** Open the DMG and drag
   `oxo-flow.app` into `/Applications` first, then launch it from there.
   Apps launched in place from a downloaded DMG are evaluated by Gatekeeper
   before they finish starting and can hang or be reported as damaged.

2. **First launch — Gatekeeper.** The release bundle is signed with an
   ad-hoc signature (no Apple Developer ID — the project cannot ship a
   Developer ID certificate from CI). Gatekeeper treats downloaded ad-hoc
   apps as untrusted, so the first launch needs one of these:

   - **Recommended** — clear the download quarantine once, then launch
     normally every time:

     ```bash
     xattr -dr com.apple.quarantine /Applications/oxo-flow.app
     open /Applications/oxo-flow.app
     ```

   - Or right-click (Control-click) `oxo-flow.app` in Finder → **Open** →
     click **Open** in the confirmation dialog. (Note: on macOS 26 the
     right-click Open bypass no longer exists for every app; the `xattr`
     command above always works.)

   If the system reports **“oxo-flow.app is damaged and cannot be opened”**
   (or “is damaged”), this is Gatekeeper rejecting the unsigned download —
   the fix is the `xattr` command above. On Apple Silicon, unsigned or
   improperly signed arm64 apps show this “damaged” error instead of the
   “unidentified developer” prompt seen on Intel. Clearing the quarantine
   attribute is the only reliable cure; enabling “Allow applications from
   anywhere” (`sudo spctl --master-disable`) alone is not sufficient.

   The downloaded DMG itself carries the quarantine attribute; the command
   above strips it from the installed app. Alternatively strip it from the
   DMG before opening it:

   ```bash
   xattr -dr com.apple.quarantine ~/Downloads/oxo-flow-*.dmg
   ```

3. **First launch may be slow** while macOS indexes the app; subsequent
   launches are instant.

### macOS specifics

- **Icon**: the bundle ships `Contents/Resources/oxo-flow.icns` (rendered
  from `logo.svg`, stored in the repo at `assets/oxo-flow.icns`) and the
  `Info.plist` declares `CFBundleIconFile`.
- **Signature**: CI ad-hoc-signs the bundle (`codesign --force --deep
  --sign -`) before creating the DMG. This makes the app internally
  consistent — its Mach-O, `Info.plist`, and resources are sealed under one
  signature — which prevents the “damaged” classification once the
  quarantine attribute is cleared. It is **not** a Developer ID signature:
  Gatekeeper still blocks quarantined downloads, and a future
  notarization pipeline (Developer ID certificate) would remove the
  quarantine workaround entirely.

## Linux (.deb / .rpm / .AppImage)

```bash
make bundle-deb    # Debian/Ubuntu package
make bundle-rpm    # RHEL/Fedora package
# or directly:
cargo bundle --release --format deb
```

```bash
sudo dpkg -i target/release/bundle/deb/oxo-flow_*.deb
oxo-flow serve --open
```

The release `.deb` / `.rpm` ship an application-menu entry with the
oxo-flow icon (`oxo-flow serve --open`), so after install you can also
launch it from the desktop environment's app menu. The `.AppImage`
carries the same entry (AppRun + `oxo-flow.desktop` + `oxo-flow.png`);
double-click the file or run `./oxo-flow-*.AppImage`.

## Windows

There is no Windows bundle yet — the release pipeline does not build one
(`cargo-bundle`'s `msi` format requires a Windows host). Windows users
run the Linux binaries under WSL2.

## Verification

After bundling, verify the packaged app serves the interface without a
source checkout (the runtime resolves `static/` relative to the
executable). In the CI bundle the real binary is `oxo-flow-bin`:

```bash
APP="target/release/bundle/macos/oxo-flow.app/Contents/MacOS/oxo-flow-bin"
"$APP" serve -p 8999 &
curl -s http://127.0.0.1:8999/ | grep -q "__OXO_BASE__" && echo "SPA OK"
```

Verify the bundle signature and icon:

```bash
codesign -dv --verbose=4 oxo-flow.app        # Signature=adhoc
plutil -lint oxo-flow.app/Contents/Info.plist
```

## GitHub Release Assets

Each tagged release publishes the desktop bundles alongside the raw
tarballs (built by CI, not by hand):

| Asset | Platform |
|---|---|
| `oxo-flow-<ver>-desktop-x86_64-apple-darwin.dmg` / `-desktop-…-app.zip` | macOS Intel (Rosetta on Apple Silicon) |
| `oxo-flow-<ver>-desktop-aarch64-apple-darwin.dmg` / `-desktop-…-app.zip` | macOS Apple Silicon |
| `oxo-flow-<ver>-desktop-amd64.deb` | Debian / Ubuntu |
| `oxo-flow-<ver>-desktop-x86_64.rpm` | RHEL / Fedora / CentOS |
| `oxo-flow-<ver>-desktop-x86_64.AppImage` | any Linux distribution |
| `oxo-flow-<ver>-<target>.tar.gz` | CLI binary, 8 targets (macOS ×2, Linux glibc/musl ×3 architectures) — for clusters, containers, and scripted installs |
| `oxo-flow-web-<ver>-<target>.tar.gz` | Standalone web-server binary (no CLI subcommands) — for deployment hosts that only serve the UI |
| `SHA256SUMS.txt` | Checksums for every asset above |

### Install from a tarball

```bash
curl -LO https://github.com/Traitome/oxo-flow/releases/download/v0.16.0/oxo-flow-v0.16.0-x86_64-unknown-linux-gnu.tar.gz
curl -LO https://github.com/Traitome/oxo-flow/releases/download/v0.16.0/SHA256SUMS.txt
sha256sum -c SHA256SUMS.txt --ignore-missing   # verify before you run it
tar xzf oxo-flow-v0.16.0-x86_64-unknown-linux-gnu.tar.gz
sudo install -m 755 oxo-flow /usr/local/bin/oxo-flow
```

Pick the target that matches the machine: `gnu` for glibc distributions,
`musl` for Alpine/static links, `armv7` for 32-bit ARM.

```bash
# Linux one-liners
sudo dpkg -i oxo-flow-*desktop*.deb          # Debian/Ubuntu
sudo rpm -i oxo-flow-*desktop*.rpm           # RHEL/Fedora
chmod +x oxo-flow-*desktop*.AppImage && ./oxo-flow-*desktop*.AppImage   # any distro
```

The AppImage runs `oxo-flow serve --open` on launch; the deb/rpm install
`/usr/bin/oxo-flow` with the SPA under `/usr/share/oxo-flow/static`
(resolved by the executable-relative frontend lookup, so no source
checkout is needed).

## Notes

- **Version**: the bundle inherits the crate version from the workspace.
- **Icon**: the app icon (`assets/oxo-flow.icns`, macOS) is rendered from
  `logo.svg` and shipped in the CI bundle. For local cargo-bundle builds,
  add `icon = ["../../assets/oxo-flow.icns"]` under
  `[package.metadata.bundle]` in `crates/oxo-flow-cli/Cargo.toml` (`.icns`
  for macOS, `.png`/`.ico` elsewhere); the bundle works without one.
- **Signing**: CI ad-hoc-signs the macOS bundle (`codesign --force --deep
  --sign -`). Downloaded ad-hoc apps still hit Gatekeeper — see the
  install steps above. A Developer ID certificate + notarization would
  remove the quarantine workaround; Linux packages should be signed with a
  GPG key.
