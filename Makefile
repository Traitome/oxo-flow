.PHONY: ci fmt clippy build test coverage bench bench-macro bench-compare audit deny aws-legacy-tripwire issue-id-tripwire secrets docs frontend-lint frontend-test schema-drift skill-drift sync-skill version-check eval-tests eval-lint eval-verify extension-test extension-integration contributors frontend-build frontend-dev dev bundle-static bundle-desktop bundle-macos bundle-deb bundle-rpm bundle-appimage help

## Run all local CI quality-gate checks. Same gates as the "Test" job in
## ci.yml, but the invocations are not identical: `test` runs single-threaded
## (--test-threads=1, deterministic locally; CI runs the default parallelism)
## and `frontend-lint` uses `npm install` where CI uses `npm ci`.
ci: fmt clippy build test schema-drift skill-drift version-check audit deny aws-legacy-tripwire issue-id-tripwire secrets docs frontend-lint eval-tests eval-lint extension-test

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

## AWS legacy-client tripwire (#658, inverted by #876): the legacy
## hyper-0.14 / rustls-0.21 / h2-0.3 stack was removed from the
## s3-storage graph by declaring aws-sdk-s3 with default-features = false
## minus `rustls` (#876), and all AWS-stack advisory ignores were deleted
## from .cargo/audit.toml and deny.toml. The gate now FAILS when the
## legacy stack re-appears in the graph (regression = re-linked), and
## fails if engine code references the legacy connector. Cargo.lock must
## be up to date for the graph probe to be meaningful.
aws-legacy-tripwire:
	@if cargo tree -p oxo-flow-core --features s3-storage -i h2@0.3.27 >/dev/null 2>&1; then \
	  echo "aws-legacy-tripwire: legacy h2 0.3 is BACK in the s3-storage graph — the #876 fix regressed; remove it (issue #658/#876)"; \
	  exit 1; \
	else \
	  echo "aws-legacy-tripwire: h2 0.3 absent from the s3-storage graph — legacy stack stays out (issue #876)"; \
	fi
	@if grep -rn "hyper_014" crates --include="*.rs"; then \
	  echo "aws-legacy-tripwire: the legacy connector is referenced in engine code — re-assess the #876 feature-level fix"; \
	  exit 1; \
	fi

## Issue-ID tripwire: user-facing marketing surfaces (docs/index.html,
## README.md, editors/vscode/README.md) must not leak internal tracker IDs —
## readers have no issue-tracker access. Detection uses issue-shaped
## patterns (`Issues? #[0-9]+`, `#N / #M`), never hex-color exclusion, which
## is blind to 3-digit IDs (#204) byte-identical to CSS hex colors. The
## docs/guide/src "(issue #NNN)" reference-doc citations are deliberately
## out of scope.
issue-id-tripwire:
	@if grep -nE 'Issues? #[0-9]+|#[0-9]+ */ *#[0-9]+' docs/index.html README.md editors/vscode/README.md; then \
	  echo "issue-id-tripwire: internal issue IDs leaked into user-facing pages — replace with descriptive text"; \
	  exit 1; \
	else \
	  echo "issue-id-tripwire: user-facing pages clean of internal issue IDs"; \
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

## Eval-harness unit tests: deterministic judge logic plus evidence-stage,
## paired-statistics, hardware-ingestion, and cohort-measurement contracts.
## Silent rot here would turn missing evidence into fabricated conclusions.
eval-tests:
	python3 -m unittest discover -s eval/scripts
	python3 -m unittest discover -s eval/frontier -p 'test_*.py'
	python3 -m unittest discover -s eval/benchmarks -p 'test_*.py'

## Eval gold consistency over eval/gold/*.csv (issue #172): fabricated
## negative names colliding with the knowledge base, gold-vs-KB version
## drift after a knowledge refresh, missing provenance evidence, and
## declared DAG edges absent from the reference's engine graph. Needs a
## built engine binary for the DAG check (cargo build --workspace).
eval-lint:
	python3 eval/scripts/check_gold.py --no-fetch-community

## Cryptographic evidence audit for the eval gold set (issue #172): verifies
## 100% hash integrity in eval/review/evidence.jsonl and expert reviewer sign-off.
eval-verify:
	python3 eval/scripts/verify_gold.py

## Run frontend Playwright e2e tests (needs the Rust server; see ci.yml).
frontend-test:
	cd frontend && npx playwright test

## VS Code extension (editors/vscode) fast gate: compile, unit tests, and a
## real `vsce package` so manifest/packaging problems fail before CI does.
## The extension has zero runtime dependencies; this needs network only for
## the first `npm ci`.
extension-test:
	cd editors/vscode && npm ci --no-audit --no-fund && npm run compile && npm run unit-test && npm run package

## VS Code extension integration tests: headless VS Code driving the locally
## built CLI (cargo build -p oxo-flow-cli). Linux needs xvfb on PATH.
extension-integration:
	cargo build -p oxo-flow-cli
	cd editors/vscode && OXOFLOW_TEST_BIN="$(CURDIR)/target/debug/oxo-flow" npm run integration-test

## Single-source rule: the CLI-embedded workflow schema must match the
## docs copy (the docs copy is canonical; `oxo-flow schema` serves the
## CLI copy — drift means users validate against an outdated schema).
schema-drift:
	diff -q crates/oxo-flow-cli/schema/oxoflow-v1.schema.json docs/schema/oxoflow-v1.schema.json >/dev/null 2>&1 || { echo "schema drift: sync crates/oxo-flow-cli/schema with docs/schema"; exit 1; }

## Single-source rule: the SKILL.md embedded in the CLI (`oxo-flow agent-docs`
## writes it into projects) must match the docs copy (canonical, served at
## /latest/SKILL/). The copy lives inside the crate because cargo
## package/publish only ships crate-local files. `make sync-skill` refreshes it.
skill-drift:
	diff -q crates/oxo-flow-cli/assets/SKILL.md docs/guide/src/SKILL.md >/dev/null 2>&1 || { echo "skill drift: run 'make sync-skill' (docs/guide/src/SKILL.md is canonical)"; exit 1; }

sync-skill:
	@cp docs/guide/src/SKILL.md crates/oxo-flow-cli/assets/SKILL.md
	@echo "synced crates/oxo-flow-cli/assets/SKILL.md from docs/guide/src/SKILL.md"

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

## Start both the API server and frontend dev server (Ctrl-C stops both).
dev: frontend-build
	@echo "Starting oxo-flow-web on :3000 and frontend on :5173..."
	@trap 'kill 0' EXIT INT TERM; \
	cd frontend && npm run dev & \
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

## List available targets with their descriptions (kept at the bottom so the
## default goal stays `ci`).
help:
	@awk 'BEGIN { desc = "" } \
		/^## / { d = substr($$0, 4); desc = (desc == "" ? d : desc " " d); next } \
		/^[a-zA-Z][a-zA-Z0-9_-]*:/ { \
			if (desc != "") { name = $$1; sub(/:.*/, "", name); printf "  %-24s %s\n", name, desc } \
			desc = "" }' $(firstword $(MAKEFILE_LIST))
