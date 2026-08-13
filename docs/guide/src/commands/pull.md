# oxo-flow pull

Fetch an oxo-flow workflow from a remote source. Two modes:

- **Bundle mode** — download a published bundle, verify every file's SHA-256
  checksum against its `manifest.json`, ready for `oxo-flow run --bundle`.
- **Repository mode** — `git clone` a workflow repository directly. No
  packaging step is needed on the publishing side: anything that is a git
  repo with an `.oxoflow` file at its root works.

## Usage

```
oxo-flow pull [OPTIONS] <URL>
```

## Description

### Supported URL Schemes

| Scheme | Mode | Example |
|---|---|---|
| GitHub Release | bundle | `gh:owner/repo@tag` — first `.tar.zst`/`.tar.gz` asset |
| HTTPS archive | bundle | `https://example.com/bundle.tar.zst` |
| Local bundle | bundle | `file:///data/bundles/pipeline.tar.zst` |
| GitHub repo | **repository** | `gh:owner/repo` — clone the default branch |
| Git URL | **repository** | `https://example.com/team/pipeline.git` |
| Local repo | **repository** | `file:///path/to/repo` (a directory) |

The distinction is deterministic: `@tag` selects the GitHub Release namespace
(bundle); without `@` (or a `.git` URL / a directory path) the repository is
cloned. Repository mode auto-discovers the workflow (`main.oxoflow` first,
else the alphabetically first `*.oxoflow`), sanity-parses it with the engine,
and prints the `oxo-flow run` command to use. Private repositories work
through your normal git credentials.

### China mirror fallback

For `github.com` clones, the official URL is tried first; if it fails, the
`ghfast.top` and `gh-proxy.com` mirrors are tried automatically in order.
Every failure is reported with a pointer to the
[China Mirrors guide](../how-to/china-mirrors.md).

---

## Options

| Option | Short | Description |
|---|---|---|
| `--output` | `-o` | Output path for the downloaded bundle / clone directory (default: derived from URL) |

## Examples


```bash
# Pull from a GitHub release (bundle mode)
oxo-flow pull gh:WangLabCSU/oxo-flow-circrna@v0.11.0

# Pull from an HTTPS URL (.tar.zst format)
oxo-flow pull https://example.com/pipelines/circrna-bundle.tar.zst

# Pull from an HTTPS URL (.tar.gz format)
oxo-flow pull https://example.com/pipelines/align-bundle.tar.gz

# Pull to a custom path
oxo-flow pull gh:user/repo@v2 -o my-pipeline.tar.zst

# Pull and execute in one step (bundle)
oxo-flow pull gh:user/repo@v1 -o repo-bundle.tar.zst && oxo-flow run --bundle repo-bundle.tar.zst -j 16 --yes

# Repository mode — no bundle required: clone + auto-discover the workflow
oxo-flow pull gh:WangLabCSU/oxo-flow-circrna
# ✓ Cloned into ./oxo-flow-circrna (workflow: main.oxoflow, 12 rules)
#   Run with: oxo-flow run ./oxo-flow-circrna/main.oxoflow
```

### Publishing with Format Selection

```bash
# Publish with zstd compression (default, smaller files)
oxo-flow publish pipeline.oxoflow

# Publish with gzip compression (universal compatibility)
oxo-flow publish pipeline.oxoflow --format tar.gz
```

## Verification

Every downloaded bundle is verified against its `manifest.json` before being
saved. If any file's SHA-256 checksum doesn't match the manifest, the download
is rejected with a clear error message showing which file failed.

## See Also

- [oxo-flow publish](publish.md) — create bundles
- [oxo-flow run](run.md) — `--bundle` flag for executing bundles
