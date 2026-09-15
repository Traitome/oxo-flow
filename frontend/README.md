# oxo-flow Web Frontend

React 19 + TypeScript single-page app for the oxo-flow web server: pipeline
editor (CodeMirror 6), DAG visualization (React Flow + d3-dag), run monitoring,
and the AI chat panel. Built with Vite; linted with ESLint and type-checked
with `tsc -b`.

## Development

```bash
npm install
npm run dev      # Vite dev server on :5173, proxies /api to localhost:3000
```

Start the API server in a second terminal:

```bash
cargo run -p oxo-flow-web   # serves the API on :3000
```

## Production build

```bash
npm run build   # type-checks, then outputs static assets to dist/
```

The server serves this `dist/` directory when `OXO_FLOW_FRONTEND_DIR` points
at it (or via `make bundle-static` for packaging into the CLI binary).

## Testing

```bash
npx playwright test   # e2e tests; requires the Rust server running on :3000
```
