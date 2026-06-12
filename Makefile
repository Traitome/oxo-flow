.PHONY: ci fmt clippy build test coverage bench audit

## Run all local CI quality-gate checks (mirrors the "Test" job in ci.yml).
## Every check must pass before calling report_progress / git push.
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
## Install: cargo install cargo-tarpaulin
coverage:
	cargo tarpaulin --workspace --out Xml --out Html --output-dir target/coverage

## Run benchmarks for performance regression tracking.
bench:

## Run macro-benchmarks (CLI-driven lifecycle, scaling, reliability).
bench-macro:
tpython3 benches/macro/suite.py --oxo-flow target/debug/oxo-flow --output benches/macro/results

## Run comparative benchmarks against Nextflow/Snakemake (requires tools).
bench-compare:
t./benches/comparative/run_comparison.sh
	cargo bench --workspace --save-baseline baseline
