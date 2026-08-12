# oxo-flow pull

Download a published oxo-flow bundle from a remote source, verify its integrity,
and save it locally ready for execution.

## Usage

```
oxo-flow pull [OPTIONS] <URL>
```

## Description

Downloads a bundle archive from the specified URL, verifies every file's
SHA-256 checksum against the bundle's `manifest.json`, and saves the verified
archive to disk. Supports `.tar.zst` (default) and `.tar.gz` formats.
The downloaded bundle can then be executed directly with `oxo-flow run --bundle`.

### Supported URL Schemes

| Scheme | Format | Example |
|---|---|---|
| GitHub Release | `gh:owner/repo@tag` | `gh:WangLabCSU/oxo-flow-circrna@v0.10.2` |
| HTTPS | `https://host/path` | `https://example.com/bundle.tar.zst` |
| HTTP | `http://host/path` | `http://example.com/bundle.tar.gz` |
| Local file | `file:///path` | `file:///data/bundles/pipeline.tar.zst` |

For `gh:` URLs, the command resolves the GitHub release by tag and downloads
the first `.tar.zst` or `.tar.gz` asset listed in the release.

---

## Options

| Option | Short | Description |
|---|---|---|
| `--output` | `-o` | Output path for the downloaded bundle (default: derived from URL) |

## Examples

```bash
# Pull from a GitHub release
oxo-flow pull gh:WangLabCSU/oxo-flow-circrna@v0.10.2

# Pull from an HTTPS URL (.tar.zst format)
oxo-flow pull https://example.com/pipelines/circrna-bundle.tar.zst

# Pull from an HTTPS URL (.tar.gz format)
oxo-flow pull https://example.com/pipelines/align-bundle.tar.gz

# Pull to a custom path
oxo-flow pull gh:user/repo@v2 -o my-pipeline.tar.zst

# Pull and execute in one step
oxo-flow pull gh:user/repo@v1 && oxo-flow run --bundle repo-bundle.tar.zst -j 16 --yes

# Pull and review resources before running
oxo-flow pull gh:user/repo@v1
oxo-flow run --bundle repo-bundle.tar.zst --yes
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
