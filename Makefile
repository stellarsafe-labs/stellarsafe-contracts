.PHONY: all build test test-wasm lint fmt check clean size fuzz

WASM := target/wasm32v1-none/release/stellarsafe_treasury.wasm
MAX_WASM_BYTES := 131072

all: check

## Build the optimized contract Wasm (requires stellar-cli >= 25.2).
build:
	stellar contract build

## Unit + scenario tests (native, fast).
test:
	cargo test --workspace

## Tests against the compiled Wasm, metered against mainnet limits.
test-wasm: build
	cargo test -p stellarsafe-treasury --features wasm-tests --test wasm

lint:
	cargo fmt --all -- --check
	cargo clippy --workspace --all-targets --all-features -- -D warnings

fmt:
	cargo fmt --all

## Fail if the Wasm exceeds the network's contract size limit.
size: build
	@size=$$(wc -c < $(WASM)); echo "wasm size: $$size bytes (limit $(MAX_WASM_BYTES))"; \
	  test $$size -le $(MAX_WASM_BYTES)

check: lint test test-wasm size

clean:
	cargo clean

## Long model-based fuzz run.
fuzz:
	PROPTEST_CASES=2000 cargo test --release -p stellarsafe-treasury contract_matches_reference_model
