.PHONY: ci fmt clippy build test coverage bench bench-macro bench-compare audit deny aws-legacy-tripwire secrets docs frontend-lint frontend-test schema-drift version-check contributors frontend-build frontend-dev dev bundle-static bundle-desktop bundle-macos bundle-deb bundle-rpm bundle-appimage

## Run all local CI quality-gate checks. Same gates as the "Test" job in
## ci.yml, but the invocations are not identical: `test` runs single-threaded
## (--test-threads=1, deterministic locally; CI runs the default parallelism)
## and `frontend-lint` uses `npm install` where CI uses `npm ci`.
ci: fmt clippy build test schema-drift version-check audit deny aws-legacy-tripwire secrets docs frontend-lint eval-tests

fmt:
	cargo fmt -- --check

clippy:
	cargo clippy --workspace --all-targets -- -D warnings

build:
	cargo build --workspace

test:
	PATH="$$(pwd)/target/debug:$$PATH" cargo test --workspace -- --test-threads=1

audit:
	cargo audit --no-fetch 2>&1 || cargo audit

## Dependency-policy gate: license allowlist, duplicate/wildcard bans and
## crate-source checks (config in deny.toml, #557).
deny:
	cargo deny check

## AWS legacy-client tripwire (#658): the RUSTSEC ignores in
## .cargo/audit.toml and deny.toml are valid only while BOTH hold — (a) the
## legacy hyper-0.14 stack is merely compiled in under the opt-in
## s3-storage feature, and (b) nothing in the engine references the legacy
## connector at runtime. This gate fails — prompting removal of the
## ignores — the day upstream drops `hyper-014` from the client defaults
## (h2 0.3 leaves the graph), and fails if someone introduces runtime
## reachability, invalidating the justification. Keep in lockstep with the
## two config files.
aws-legacy-tripwire:
	@if cargo tree -p oxo-flow-core --features s3-storage -i h2@0.3.27 >/dev/null 2>&1; then \
	  echo "aws-legacy-tripwire: legacy stack still linked — advisory ignores remain valid"; \
	else \
	  echo "aws-legacy-tripwire: h2 0.3 is GONE from the s3-storage graph — remove the RUSTSEC ignores from .cargo/audit.toml and deny.toml now (issue #658)"; \
	  exit 1; \
	fi
	@if grep -rn "hyper_014" crates --include="*.rs"; then \
	  echo "aws-legacy-tripwire: the legacy connector is referenced in engine code — the advisory ignores no longer hold; remove them and re-assess (issue #658)"; \
	  exit 1; \
	fi

## Secret-scanning gate over the git history and working tree (config in
## .gitleaks.toml, #557). Run from the repo root: gitleaks auto-discovers
## the config there but not above a --source directory.
secrets:
	gitleaks detect

## Build rustdoc for the workspace with warnings denied (#557). Doc builds
## were never gated, so broken intra-doc links and name collisions rotted
## silently; -D warnings makes this a real gate.
docs:
	RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps

## Lint and type-check the frontend SPA (same gate as the "Frontend" job in
## ci.yml; uses `npm install` rather than CI's `npm ci`).
frontend-lint:
	cd frontend && npm install --no-audit --no-fund && npm run lint

## Eval-harness unit tests: the deterministic judge logic in eval/scripts
## (runner.py scoring/matching) feeds AI-quality benchmark decisions, so a
## silent rot would produce garbage numbers (audit #673).
eval-tests:
	python3 -m unittest discover -s eval/scripts

## Run frontend Playwright e2e tests (needs the Rust server; see ci.yml).
frontend-test:
	cd frontend && npx playwright test

## Single-source rule: the CLI-embedded workflow schema must match the
## docs copy (the docs copy is canonical; `oxo-flow schema` serves the
## CLI copy — drift means users validate against an outdated schema).
schema-drift:
	diff -q crates/oxo-flow-cli/schema/oxoflow-v1.schema.json docs/schema/oxoflow-v1.schema.json >/dev/null 2>&1 || { echo "schema drift: sync crates/oxo-flow-cli/schema with docs/schema"; exit 1; }

## Single-source rule: every place the project version appears — Cargo.toml
## ([package], [workspace.package], dep pins), both lockfiles, CITATION.cff,
## the Dockerfile ARG default, the excluded desktop crate, the frontend
## manifests, README and the docs — must agree with [workspace.package].
## scripts/bump-version.sh is the one implementation; the release job runs the
## same script with --set, so local and CI cannot diverge.
version-check:
	@bash scripts/bump-version.sh --check

## Generate code coverage report (requires cargo-tarpaulin).
coverage:
	cargo tarpaulin --workspace --out Xml --out Html --output-dir target/coverage

## Run micro-benchmarks for performance regression tracking. --save-baseline
## is a criterion flag, so it must come after `--` (cargo bench rejects it).
bench:
	cargo bench -p oxo-flow-core -- --save-baseline baseline

## Run macro-benchmarks (CLI-driven lifecycle, scaling, reliability).
bench-macro:
	python3 benches/macro/suite.py --oxo-flow target/debug/oxo-flow --output benches/macro/results

## Run comparative benchmarks against Nextflow/Snakemake (requires tools).
bench-compare:
	./benches/comparative/run_comparison.sh

## Build the frontend SPA from source.
frontend-build:
	cd frontend && npm install && npm run build

## Start the frontend dev server (port 5173) with API proxy to localhost:3000.
frontend-dev:
	cd frontend && npm run dev

## Start both the API server and frontend dev server.
dev: frontend-build
	@echo "Starting oxo-flow-web on :3000 and frontend on :5173..."
	@cd frontend && npm run dev & \
	cd crates/oxo-flow-web && cargo run -- --port 3000

## List human contributors from git history (excludes bots and AI tools).
contributors:
	@echo "Human contributors (from git log):"
	@git log --format="%aN" --all | grep -v "Claude\|noreply\|bot\|Copilot" | sort -u

# ── Desktop packaging (docs/guide/src/how-to/desktop-app.md) ──────────────
# Single source (#556): bump-version.sh --print is the one implementation.
VERSION := $(shell bash scripts/bump-version.sh --print)

# The SPA build output is copied into the CLI crate so the bundle carries it
# without ".." resource paths (cargo-bundle mangles those). Frontend must be
# built first — assets are gitignored build artifacts.
bundle-static:
	@cd frontend && npm run build
	@rm -rf crates/oxo-flow-cli/static
	@cp -r crates/oxo-flow-web/static crates/oxo-flow-cli/static

# Native-window desktop shell (wry + tao) around the embedded web server.
# Lives outside the cargo workspace (GUI toolchains; see crates/
# oxo-flow-desktop/Cargo.toml), so it builds with its own invocation.
bundle-desktop:
	@cd frontend && npm run build
	cd crates/oxo-flow-desktop && cargo build --release
	@echo "→ crates/oxo-flow-desktop/target/release/oxo-flow-desktop"

# Same packaging path as the release CI (scripts/package-macos-app.sh): the
# bundle executable is the desktop shell itself, not the CLI binary.
bundle-macos: bundle-static bundle-desktop
	bash scripts/package-macos-app.sh '' "v$(VERSION)" dist

bundle-deb: bundle-static
	cd crates/oxo-flow-cli && cargo bundle --release --format deb

bundle-rpm: bundle-static
	cd crates/oxo-flow-cli && cargo bundle --release --format rpm

bundle-appimage: bundle-static
	cd crates/oxo-flow-cli && cargo bundle --release --format appimage
