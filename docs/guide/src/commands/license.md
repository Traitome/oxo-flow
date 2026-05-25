# `oxo-flow license`

Verify or display license status.

---

## Usage

```
oxo-flow license [LICENSE_PATH]
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
#   Message: Academic license active - free for non-commercial use.
```

### Verify a commercial license file

```bash
oxo-flow license /path/to/license.key
```

---

## Notes

- oxo-flow ships with a default academic license for non-commercial use.
- Commercial use requires a paid license file. Contact Traitome for details.
- The license check runs automatically on `oxo-flow serve` startup.
