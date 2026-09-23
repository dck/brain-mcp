.PHONY: build release install uninstall test lint fmt check clean

build:
	cargo build --workspace

release:
	cargo build --release

install: release
	cargo install --path brain-cli

uninstall:
	cargo uninstall brain-cli

test:
	cargo test --workspace

lint:
	cargo clippy --workspace -- -D warnings

fmt:
	cargo fmt --all

check: fmt lint test

clean:
	cargo clean
