# `oxo-flow license`

Verify or display license status.

---

## Usage

```
oxo-flow license [OPTIONS] [LICENSE_PATH]
```

---

## Arguments

| Argument | Description |
|---|---|
| `[LICENSE_PATH]` | Path to a commercial license file to verify. If omitted, displays the current license status. |

---

## Examples

### Check current license status

```bash
oxo-flow license
# Output:
# License status:
#   Status:  Valid (academic)
#   Issued:  Public Academic Test License (any academic user)
#   Message: Academic license active - free for non-commercial use. Commercial use requires a paid license file.
```

### Verify a commercial license file

```bash
oxo-flow license /path/to/license.key
```

---

## Options

| Option | Description |
|--------|-------------|
| `-v, --verbose` | Enable verbose (debug-level) logging |
| `--quiet` | Suppress non-essential output (errors only) |
| `--no-color` | Disable colored output |

!!! note "No `--json` output"

    `license` always prints human-readable text; the global `--json` flag
    is not honored here.

## Notes

- oxo-flow ships with a default academic license for non-commercial use.
- Commercial use requires a paid license file. Contact Traitome for details.
- License status is also exposed by the web server via `GET /api/license` (see [oxo-flow serve](serve.md)).
