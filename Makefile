.PHONY: all build clean indent

all: build

build:
	cargo build --release

clean:
	cargo clean

indent:
	cargo fmt
	cargo clippy --all-targets -- -D warnings
