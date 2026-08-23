.PHONY: ci fmt clippy build test coverage bench bench-macro bench-compare audit contributors

## Run all local CI quality-gate checks (mirrors the "Test" job in ci.yml).
ci: fmt clippy build test schema-drift audit

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

## Single-source rule: the CLI-embedded workflow schema must match the
## docs copy (the docs copy is canonical; `oxo-flow schema` serves the
## CLI copy — drift means users validate against an outdated schema).
schema-drift:
	diff -q crates/oxo-flow-cli/schema/oxoflow-v1.schema.json docs/schema/oxoflow-v1.schema.json >/dev/null 2>&1 || { echo "schema drift: sync crates/oxo-flow-cli/schema with docs/schema"; exit 1; }

## Generate code coverage report (requires cargo-tarpaulin).
coverage:
	cargo tarpaulin --workspace --out Xml --out Html --output-dir target/coverage

## Run micro-benchmarks for performance regression tracking.
bench:
	cargo bench --workspace --save-baseline baseline

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

# ── Desktop packaging (cargo-bundle, docs/how-to/desktop-app.md) ──────────
# The SPA build output is copied into the CLI crate so the bundle carries it
# without ".." resource paths (cargo-bundle mangles those). Frontend must be
# built first — assets are gitignored build artifacts.
bundle-static:
	@cd frontend && npm run build
	@rm -rf crates/oxo-flow-cli/static
	@cp -r crates/oxo-flow-web/static crates/oxo-flow-cli/static

bundle-macos: bundle-static
	cd crates/oxo-flow-cli && cargo bundle --release --format osx
	cd crates/oxo-flow-cli && cargo bundle --release --format dmg

bundle-deb: bundle-static
	cd crates/oxo-flow-cli && cargo bundle --release --format deb

bundle-rpm: bundle-static
	cd crates/oxo-flow-cli && cargo bundle --release --format rpm

bundle-appimage: bundle-static
	cd crates/oxo-flow-cli && cargo bundle --release --format appimage
