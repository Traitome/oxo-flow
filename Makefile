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
