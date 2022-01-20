build:
	cargo build-bpf

analyze:
    soteria -c -analyzeAll .

deploy_and_test: build
	solana program deploy ./target/deploy/strike_wallet.so
	cargo test-bpf

test:
	cargo test-bpf -- --nocapture
