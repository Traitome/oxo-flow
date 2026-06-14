# License Model

oxo-flow uses a per-crate licensing model with dual licensing for the web server.

## License by Crate

| Crate | License | Usage |
|-------|---------|-------|
| `oxo-flow-core` | Apache 2.0 | Free for all uses |
| `oxo-flow-cli` | Apache 2.0 | Free for all uses |
| `oxo-flow-web` | Dual License | Academic: free / Commercial: paid |

## oxo-flow-web Dual License

### Academic Use (LICENSE-ACADEMIC)

Free for:
- Academic research at universities and non-profit research institutions
- Teaching and educational purposes
- Non-commercial government research

Under the academic license, you may:
- Use, modify, and distribute the software freely
- Contribute modifications back to the project
- Run the web server for academic research groups

### Commercial Use (LICENSE-COMMERCIAL)

Requires a paid license for:
- Pharmaceutical and biotech companies
- Clinical diagnostic laboratories
- Commercial bioinformatics service providers
- Any for-profit entity using the software in revenue-generating activities

Contact: **Shixiang Wang <w_shixiang@163.com>**

## License Verification

The web server verifies its license at startup:

1. Checks `OXO_FLOW_LICENSE` environment variable
2. Checks platform config directory (`io.traitome.oxo-flow/license.oxo.json`)
3. Checks legacy path (`~/.config/oxo-flow/license.oxo.json`)
4. Falls back to embedded academic license (default)

```bash
# Set commercial license
export OXO_FLOW_LICENSE=/path/to/license.oxo.json

# Upload via API
curl -X POST http://localhost:8777/api/license/upload \
  -H "Content-Type: application/json" \
  -d '{"license_data": "..."}'

# Check status
curl http://localhost:8777/api/license
```

## License Visibility

The license is prominently displayed in three locations:

1. **Startup banner** — Printed to stderr on server start
2. **Web footer** — Persistent footer on every page
3. **API response header** — `X-OxoFlow-License` on all HTTP responses

```
X-OxoFlow-License: oxo-flow-core,oxo-flow-cli:Apache-2.0; oxo-flow-web:Dual(Academic|Commercial)
X-OxoFlow-Version: 0.8.0
```

## Key Principles

- **Soft constraint**: Software functionality is NOT restricted by license status
  (no DRM, no feature locks)
- **Prominent notice**: The license is always visible — banner, footer, headers
- **No enforcement**: License verification is informational, not restrictive.
  The academic community is trusted to comply.

## Contributing

By contributing to oxo-flow, you agree that your contributions will be licensed
under the same terms as the crate you contribute to:

- `oxo-flow-core` and `oxo-flow-cli`: Apache 2.0
- `oxo-flow-web`: Dual license (Academic + Commercial)
