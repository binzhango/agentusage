.PHONY: fmt check test lint package build ci

fmt:
	cargo fmt --all -- --check

check:
	cargo check --locked

test:
	cargo test --locked

lint:
	cargo clippy --all-targets --all-features -- -D warnings

package:
	cargo package --locked --allow-dirty

build:
	cargo build --release --locked

ci: fmt check test lint package
