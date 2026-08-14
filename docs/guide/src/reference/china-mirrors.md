# China Network Mirrors

Reachability findings for the package and image mirrors oxo-flow users on
mainland-China networks rely on: conda/bioconda channels, PyPI, the Rust
toolchain index, and Docker registry mirrors.

These results are a **snapshot, not a contract** — mirror policies change
within hours (see the TUNA rows below: a 302 delegation in one probe, 403
in the next, on the same day). Before writing mirror configuration, re-run
the probe script on the target machine:

```bash
bash scripts/mirror-probe.sh
```

## Snapshot (2026-08-14, dev machine)

Method: two rounds of `curl` checks (status + latency; redirects followed).
Docker endpoints use the `/v2/` challenge — **401 = reachable** (auth
required), `000` = unreachable, `403` = blocked.

| Endpoint | Morning probe | Afternoon probe | Verdict |
|---|---|---|---|
| USTC bioconda | 200 | 200 | ✅ usable |
| TUNA bioconda | 302 → delegates to `cmcc.mirrors.ustc.edu.cn` | 403 | ⚠️ unstable today |
| Aliyun bioconda | 403 | 403 | ❌ geo-blocked from this egress |
| Tencent bioconda | 404 (channel root) | 404 | ❌ path discontinued |
| USTC conda-forge | 200 | 200 | ✅ usable (slow cold start) |
| TUNA conda-forge | 302 → delegates to USTC | 403 | ⚠️ unstable today |
| TUNA PyPI | 200 | 403 | ⚠️ unstable today |
| USTC PyPI | 301 → `/pypi/simple/` 200 | 301 → 200 | ✅ usable |
| Aliyun PyPI | 403 | 403 | ❌ geo-blocked from this egress |
| rsproxy.cn | 200 | 200 | ✅ usable (sparse index verified) |
| crates.io | 403 | 403 | ❌ Cloudflare blocks this egress — rsproxy required |
| USTC / TUNA / Tencent docker mirrors | unreachable | unreachable | ❌ all three discontinued |
| `docker.1ms.run` | 401 challenge | 401 challenge | ✅ only live mirror found |
| docker hub (control) | 401 challenge | 401 challenge | ✅ direct reachable |

## Recommended stack (for this environment)

- **conda / bioconda / conda-forge** (incl. pixi `[mirrors]`): **USTC**
  (`https://mirrors.ustc.edu.cn/anaconda/cloud/{bioconda,conda-forge}`) —
  TUNA delegates to USTC when it works at all.
- **PyPI** (incl. pixi `[pypi-config]`): **TUNA** when reachable, else
  **USTC** (`https://mirrors.ustc.edu.cn/pypi/simple/`).
- **Rust toolchain / crates**: **rsproxy.cn** (crates.io itself is blocked
  from this egress; the sparse index must point at rsproxy).
- **Docker**: `https://docker.1ms.run` — the three university mirrors are
  discontinued; do not copy old tutorials that reference them.

## Caveats

- The probe runs from the developer machine, whose DNS resolves to the
  `198.18.0.0/16` range (RFC 2544) — traffic egresses through a TUN-style
  proxy in fake-IP mode. Results therefore reflect real usability *from
  this environment*, not bare carrier egress; a geo-gated mirror (Aliyun)
  reporting 403 here does not prove it is dead from a mainland IP.
- Single-machine, single-day snapshot: latency and availability drift.
- The issue thread lives in
  [#67](https://github.com/Traitome/oxo-flow/issues/67) (China network
  environment checklist).
