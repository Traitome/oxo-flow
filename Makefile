.PHONY: ci fmt clippy build test coverage bench bench-macro bench-compare audit

## Run all local CI quality-gate checks (mirrors the "Test" job in ci.yml).
ci: fmt clippy build test audit

fmt:
	cargo fmt -- --check

clippy:
	cargo clippy --workspace -- -D warnings

build:
	cargo build --workspace

test:
	PATH="$$(pwd)/target/debug:$$PATH" cargo test --workspace

audit:
	cargo audit

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
