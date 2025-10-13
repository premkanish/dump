.PHONY: all build test clean run-engine run-terminal docker fmt clippy bench

all: build

build:
	cargo build --release --all

test:
	cargo test --all

clean:
	cargo clean
	rm -rf deploy

run-engine:
	cargo run --release -p engine

run-terminal:
	cargo run --release -p terminal

docker-engine:
	docker build -t hft-engine -f Dockerfile.engine .

docker-terminal:
	docker build -t hft-terminal -f Dockerfile.terminal .

fmt:
	cargo fmt --all

clippy:
	cargo clippy --all-targets --all-features -- -D warnings

bench:
	cargo bench

install:
	cargo install --path crates/engine
	cargo install --path apps/terminal

dev-engine:
	RUST_LOG=debug cargo run -p engine

dev-terminal:
	RUST_LOG=debug cargo run -p terminal

check:
	cargo check --all

watch:
	cargo watch -x 'check --all' -x 'test --all'
