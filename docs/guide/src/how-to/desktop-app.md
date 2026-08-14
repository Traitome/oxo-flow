# Desktop App Packaging

The web interface ships as a **single product file** on the desktop: the
`oxo-flow` binary already contains the CLI, the workflow engine, the web
server, and the SPA frontend. `cargo-bundle` wraps it into the native app
formats below — no browser install surprises, no separate server process.

## How it works

```
oxo-flow.app (macOS) / oxo-flow.deb (Linux)
└── oxo-flow                # the same binary as the CLI
    └── oxo-flow serve --open   # starts the local web server
                                # and opens the default browser
    └── Resources/static/       # SPA assets (resolved relative to
                                # the executable at runtime)
```

`--open` opens the interface in the system browser. Rendering stays in the
browser engine: the DAG canvas (React Flow) and CodeMirror editor are
mature browser technologies, and the browser gives users their own
extensions, password managers, and devtools. A native-webview shell
(Tauri) is a possible future enhancement for tray/notification integration,
not a prerequisite for the product experience.

## Prerequisites

1. Build the frontend first — the SPA assets are build artifacts and are
   NOT committed:

   ```bash
   cd frontend && npm install && npm run build && cd ..
   ```

2. Install cargo-bundle once:

   ```bash
   cargo install cargo-bundle --locked
   ```

## macOS (.app + .dmg)

```bash
make bundle-macos
# → target/release/bundle/macos/oxo-flow.app
# → target/release/bundle/macos/oxo-flow.dmg (double-click to install)
```

The `.app` is self-contained: drag it to /Applications, double-click, and
the interface opens in your browser. Data (SQLite + workspace) lives in
the directory you launch from; launch from a project or home directory.

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

## Windows (.msi)

```bash
cargo bundle --release --format msi
```

## Verification

After bundling, verify the packaged app serves the interface without a
source checkout (the runtime resolves `static/` relative to the
executable):

```bash
APP="target/release/bundle/macos/oxo-flow.app/Contents/MacOS/oxo-flow"
"$APP" serve -p 8999 &
curl -s http://127.0.0.1:8999/ | grep -q "__OXO_BASE__" && echo "SPA OK"
```

## GitHub Release Assets

Each tagged release publishes the desktop bundles alongside the raw
tarballs (built by CI, not by hand):

| Asset | Platform |
|---|---|
| `oxo-flow-<ver>-x86_64-apple-darwin.dmg` / `-app.zip` | macOS Intel (Rosetta on Apple Silicon) |
| `oxo-flow-<ver>-aarch64-apple-darwin.dmg` / `-app.zip` | macOS Apple Silicon |
| `oxo-flow-<ver>-amd64.deb` | Debian / Ubuntu |
| `oxo-flow-<ver>-x86_64.rpm` | RHEL / Fedora / CentOS |
| `oxo-flow-<ver>-x86_64.AppImage` | any Linux distribution |

```bash
# Linux one-liners
sudo dpkg -i oxo-flow-*.deb          # Debian/Ubuntu
sudo rpm -i oxo-flow-*.rpm           # RHEL/Fedora
chmod +x oxo-flow-*.AppImage && ./oxo-flow-*.AppImage   # any distro
```

The AppImage runs `oxo-flow serve --open` on launch; the deb/rpm install
`/usr/bin/oxo-flow` with the SPA under `/usr/share/oxo-flow/static`
(resolved by the executable-relative frontend lookup, so no source
checkout is needed).

## Notes

- **Version**: the bundle inherits the crate version from the workspace.
- **Icon**: add `icon = [...]` under `[package.metadata.bundle]` in
  `crates/oxo-flow-cli/Cargo.toml` (`.icns` for macOS, `.png`/`.ico`
  elsewhere) when a brand icon is ready; the bundle works without one.
- **Signing/notarization**: unsigned builds trigger Gatekeeper on macOS —
  for distribution, sign with an Apple Developer certificate and notarize;
  Linux packages should be signed with a GPG key.
