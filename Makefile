.PHONY: ci fmt clippy build test

## Run all local CI quality-gate checks (mirrors the "Test" job in ci.yml).
## Every check must pass before calling report_progress / git push.
ci: fmt clippy build test

fmt:
	cargo fmt -- --check

clippy:
	cargo clippy --workspace -- -D warnings

build:
	cargo build --workspace

test:
	cargo test --workspace
