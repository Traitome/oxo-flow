# China Mirrors Configuration

This guide helps users in China configure package mirrors for faster downloads
when using oxo-flow's environment management and dependency systems.

## Overview

oxo-flow relies on several package ecosystems that may be slow to access from
mainland China. Configuring mirrors significantly speeds up environment creation,
container builds, and dependency resolution.

| Ecosystem | Purpose | Primary Mirror |
|-----------|---------|---------------|
| Conda / Mamba | Bioinformatics tool environments | Tsinghua Tuna |
| PyPI (pip) | Python packages | Tsinghua Tuna |
| Pixi | Multi-language environments | Tsinghua Tuna |
| Docker | Container images | USTC / Tsinghua |
| Cargo (Rust) | Rust crates index | RsProxy / Tsinghua |
| Git (GitHub) | Advisory database, sources | SSH first, then ghfast.top / gh-proxy |

---

## Conda / Mamba

Copy this to `~/.condarc`:

```yaml
channels:
  - conda-forge
  - bioconda
  - defaults
show_channel_urls: true
default_channels:
  - https://mirrors.tuna.tsinghua.edu.cn/anaconda/pkgs/main
  - https://mirrors.tuna.tsinghua.edu.cn/anaconda/pkgs/r
custom_channels:
  conda-forge: https://mirrors.tuna.tsinghua.edu.cn/anaconda/cloud
  bioconda: https://mirrors.tuna.tsinghua.edu.cn/anaconda/cloud
```

Alternative mirrors:

- **USTC**: `https://mirrors.ustc.edu.cn/anaconda`
- **Aliyun**: `https://mirrors.aliyun.com/anaconda`
- **Tencent**: `https://mirrors.cloud.tencent.com/anaconda`

---

## Pip (PyPI)

Copy this to `~/.config/pip/pip.conf` (Linux/macOS) or `%APPDATA%\pip\pip.ini` (Windows):

```ini
[global]
index-url = https://mirrors.tuna.tsinghua.edu.cn/pypi/web/simple
```

Alternative: `https://mirrors.aliyun.com/pypi/simple/`

---

## Pixi

Pixi uses both conda and PyPI channels. Configure via `~/.config/pixi/config.toml`:

```toml
[pypi-config]
index-url = "https://mirrors.tuna.tsinghua.edu.cn/pypi/web/simple"

[mirrors]
"https://conda.anaconda.org/conda-forge" = [
    "https://mirrors.tuna.tsinghua.edu.cn/anaconda/cloud/conda-forge"
]
"https://conda.anaconda.org/bioconda" = [
    "https://mirrors.tuna.tsinghua.edu.cn/anaconda/cloud/bioconda"
]
"https://repo.anaconda.com/pkgs/main" = [
    "https://mirrors.tuna.tsinghua.edu.cn/anaconda/pkgs/main"
]
```

> **Note:** pixi `[mirrors]` values must be **arrays** of strings — a plain
> string fails to parse.

---

## Docker

> **Warning:** the long-standing university registry mirrors
> (`docker.mirrors.tuna.tsinghua.edu.cn`, `docker.mirrors.ustc.edu.cn`) were
> discontinued in 2024. Community mirrors change frequently — check a
> current, community-maintained mirror list before relying on any URL.

The most stable approach is a per-container HTTP proxy in
`~/.docker/config.json`:

```json
{
  "proxies": {
    "default": {
      "httpProxy": "http://127.0.0.1:7890",
      "httpsProxy": "http://127.0.0.1:7890"
    }
  }
}
```

---

## Cargo (Rust)

Cargo uses a sparse index by default (since Rust 1.68). For crates.io mirror:

Add to `~/.cargo/config.toml`:

```toml
[source.crates-io]
replace-with = 'rsproxy'

[source.rsproxy]
registry = 'sparse+https://rsproxy.cn/index/'

[registries.rsproxy]
index = 'sparse+https://rsproxy.cn/index/'

[net]
git-fetch-with-cli = true
```

For GitHub-based crates, configure git URL rewriting globally:

```bash
# Use a proxy for GitHub access
git config --global url."https://ghfast.top/https://github.com/".insteadOf "https://github.com/"
```

This also speeds up `cargo audit` advisory database updates.
See the [Git (GitHub)](#git-github) section below for more proxy options and SSH fallback.

---

## Git (GitHub)

**Built-in fallback**: when oxo-flow clones a `github.com` repository itself
(`oxo-flow pull gh:owner/repo`, `oxo-flow run gh:owner/repo[@ref]`), it tries
the official URL first and then falls back to the `ghfast.top` and
`gh-proxy.com` mirrors automatically — no configuration needed.

For your own manual `git clone` commands, when HTTPS-based GitHub proxies
(like `ghfast.top`) are slow or unreliable, try using SSH as a first
alternative — SSH often bypasses the same network issues that affect HTTPS:

```bash
git clone git@github.com:Traitome/oxo-flow.git
```

To convert an existing HTTPS remote to SSH:

```bash
git remote set-url origin git@github.com:Traitome/oxo-flow.git
```

You can also configure Git to always rewrite HTTPS URLs to SSH globally:

```bash
git config --global url."git@github.com:".insteadOf "https://github.com/"
```

If SSH is blocked, fall back to HTTPS proxies:

```bash
git config --global url."https://ghfast.top/https://github.com/".insteadOf "https://github.com/"
```

Common GitHub proxies:

| Proxy URL | Notes |
|-----------|-------|
| `https://ghfast.top/https://github.com` | Fast, reliable |
| `https://gh-proxy.com/https://github.com` | General purpose |

---

## Verifying Configuration

After configuring mirrors, verify they work:

```bash
# Test conda
conda create -n test-mirror fastqc --dry-run

# Test pixi
pixi init test-mirror && cd test-mirror && pixi add fastqc --dry-run

# Test pip
pip install --dry-run numpy

# Test docker
docker pull hello-world

# Test cargo
cargo search ripgrep
```

---

## Per-Workflow Configuration

oxo-flow reads environment specs from per-workflow conda/pixi files.
These files can include channel configuration inline:

```yaml
# envs/fastp.yaml
name: fastp-env
channels:
  - https://mirrors.tuna.tsinghua.edu.cn/anaconda/cloud/bioconda
  - https://mirrors.tuna.tsinghua.edu.cn/anaconda/cloud/conda-forge
dependencies:
  - fastp=0.23.4
```

This ensures reproducibility regardless of the user's global mirror settings.
