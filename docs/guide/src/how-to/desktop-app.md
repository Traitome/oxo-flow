# Desktop App Packaging

The desktop app ships as a **single product file**: the `oxo-flow-desktop`
binary contains the workflow engine, the web server, and the SPA frontend
(through `oxo-flow-web`), and renders the interface in a **native OS
webview window** — no external browser involved.

## How it works

```
oxo-flow.app (macOS)
└── Contents/MacOS/oxo-flow      # the desktop shell binary (wry + tao)
    └── Contents/Resources/static/       # SPA assets, served by the shell
    └── Contents/Resources/oxo-flow.icns # app icon
```

On launch the shell binary:

1. **anchors its working directory** to `~/.oxo-flow/desktop` — the
   per-user data directory where the SQLite database, logs, and workspace
   files live (see below);
2. **takes a single-instance lock** in that directory (a second launch
   fails fast with a clear error instead of corrupting the database);
3. **picks a free loopback port** and starts the axum web server
   (`oxo-flow-web`, personal mode — loopback-only) on a private tokio
   runtime;
4. **waits for the listener** to accept TCP connections (so the first page
   load never races server startup), then
5. **opens a native webview window** (WKWebView on macOS, WebView2 on
   Windows, WebKitGTK on Linux — via the `wry` crate) pointed at the
   server URL;
6. **exits when the window closes**, shutting the embedded server down.

Closing the window is the app lifecycle: there is no background server
process left behind. If the embedded server crashes, the window closes
and the app exits non-zero instead of freezing on a stale page. A
watchdog task races the server against `SIGINT`/`SIGTERM`, so `kill` and
Ctrl-C also shut the app down cleanly — including a signal arriving
during the slow cold GTK/webkit startup on Linux, which exits gracefully
instead of failing with a spurious startup error.

## Data directory and lifecycle

All desktop state lives in **`~/.oxo-flow/desktop`** (following the CLI's
`~/.oxo-flow` convention):

| Path | Contents |
|---|---|
| `~/.oxo-flow/desktop/oxo-flow.db` (+ `-wal`/`-shm`) | SQLite database |
| `~/.oxo-flow/desktop/logs/` | server and audit logs |
| `~/.oxo-flow/desktop/workspace/` | run workspaces |
| `~/.oxo-flow/desktop/oxo-flow.desktop.lock` | single-instance lock |

The lock is an advisory `flock` held for the process lifetime — the OS
releases it automatically if the app crashes, so there is no stale-lock
cleanup. Closing the window sends the process a `SIGTERM`, which runs the
server's graceful shutdown (flush audit logs, finish in-flight requests)
before exit; SIGTERM/SIGINT from outside do the same.

Two quality-of-life details:

- **Finder launches `.app` bundles with `cwd=/`**, where the server cannot
  create its SQLite database. The shell anchors the working directory to
  the data directory above before anything starts — so state never
  scatters across whatever directory the app happened to be launched from.
- **External links open in the system browser.** A navigation handler
  confines the app window to the app's own loopback origin; anything else
  (the GitHub/docs links in the UI) is handed to `open` / `xdg-open` /
  `start`. `target="_blank"` links and `window.open` calls are routed the
  same way. The window itself never navigates away from the interface.

Rendering stays in the OS webview (not a bundled browser, not Electron):
the DAG canvas (React Flow) and CodeMirror editor run on the platform's
own engine, exactly the engine the release server serves to.

## Prerequisites

1. Build the frontend first — a prebuilt SPA ships in
   `crates/oxo-flow-web/static/`, but rebuild it to bundle the latest UI:

   ```bash
   cd frontend && npm install && npm run build && cd ..
   ```

2. The desktop crate is **excluded from the cargo workspace** (tao/wry
   need GUI toolchains — Xcode SDKs, WebKitGTK — that headless builds and
   CI test jobs must not pull in). Build it with its own cargo invocation:

   ```bash
   cd crates/oxo-flow-desktop
   cargo build --release
   # → target/release/oxo-flow-desktop
   ```

Platform notes: macOS needs the Xcode CLIs (`xcode-select --install`);
Linux needs WebKitGTK 4.1 development files
(`libwebkit2gtk-4.1-dev` on Debian/Ubuntu) and, for Wayland, the usual
GTK scaling env vars; Windows needs the WebView2 runtime (preinstalled
on Windows 10/11) and MSVC Build Tools.

## macOS (.app + .dmg)

The GitHub release ships a hand-rolled `.app` + `.dmg` built by CI
(desktop-shell binary, icon, SPA, and ad-hoc code signature included).
To run the shell locally without packaging:

```bash
cd crates/oxo-flow-desktop && cargo run --release
```

To assemble the same bundle CI builds (`.app` + ad-hoc signature + DMG):

```bash
APP=oxo-flow.app
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp crates/oxo-flow-desktop/target/release/oxo-flow-desktop \
   "$APP/Contents/MacOS/oxo-flow"
cp -r crates/oxo-flow-web/static "$APP/Contents/Resources/static"
cp assets/oxo-flow.icns "$APP/Contents/Resources/oxo-flow.icns"
codesign --force --deep --sign - "$APP"
hdiutil create -volname "oxo-flow" -srcfolder "$APP" -ov -format UDZO oxo-flow.dmg
```

The `.app` is self-contained: drag it to /Applications, double-click, and
the interface opens in a native window. Data (SQLite + workspace) lives
in `$HOME` (see the `cwd=/` note above).

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

The Linux desktop entries (deb/rpm/AppImage) still launch
`oxo-flow serve --open`, which opens the interface in the system browser —
the release packaging for the native-window shell currently covers macOS.
Install `libwebkit2gtk-4.1-dev` and run the desktop crate directly if you
want the windowed shell on Linux today (the crate itself is
platform-independent; verified on X11/Xvfb, Linux):

```bash
cd crates/oxo-flow-desktop && cargo run --release
```

## Windows

There is no Windows bundle yet — the release pipeline does not build one
(`cargo-bundle`'s `msi` format requires a Windows host). Windows users run
the Linux binaries under WSL2. The desktop crate itself compiles for
Windows (wry uses WebView2) but is not packaged by CI yet.

## Verification

After packaging, verify the shell starts its server and serves the SPA
without a source checkout (the shell resolves `static/` relative to the
executable, same lookup as the standalone server):

```bash
APP="oxo-flow.app/Contents/MacOS/oxo-flow"
"$APP" &            # opens a native window; server on an ephemeral port
lsof -p $! | grep -m1 TCP     # shows the loopback listener
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
| `oxo-flow-<ver>-desktop-amd64.deb` | Debian / Ubuntu (menu entry opens the system browser; native window via the desktop crate) |
| `oxo-flow-<ver>-desktop-x86_64.rpm` | RHEL / Fedora / CentOS (menu entry opens the system browser; native window via the desktop crate) |
| `oxo-flow-<ver>-desktop-x86_64.AppImage` | any Linux distribution (menu entry opens the system browser; native window via the desktop crate) |
| `oxo-flow-<ver>-<target>.tar.gz` | CLI binary, 8 targets (macOS ×2, Linux glibc/musl ×3 architectures) — for clusters, containers, and scripted installs |
| `oxo-flow-web-<ver>-<target>.tar.gz` | Standalone web-server binary (no CLI subcommands) — for deployment hosts that only serve the UI |
| `SHA256SUMS.txt` | Checksums for every asset above |

### Install from a tarball

```bash
curl -LO https://github.com/Traitome/oxo-flow/releases/download/v0.17.0/oxo-flow-v0.17.0-x86_64-unknown-linux-gnu.tar.gz
curl -LO https://github.com/Traitome/oxo-flow/releases/download/v0.17.0/SHA256SUMS.txt
sha256sum -c SHA256SUMS.txt --ignore-missing   # verify before you run it
tar xzf oxo-flow-v0.17.0-x86_64-unknown-linux-gnu.tar.gz
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

- **Version**: the desktop crate repeats the workspace version
  (`0.17.0`) by value — it is outside the workspace, so it does not
  inherit `[workspace.package]`; bump it together with the rest on
  release.
- **Data directory**: the desktop app keeps all state in
  `~/.oxo-flow/desktop` (database, logs, workspace, single-instance
  lock) — see the table above. The CLI's `oxo-flow serve` and the
  desktop shell deliberately do not share a data directory, so both can
  run side by side (e.g. while comparing a release install with a dev
  build).
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
