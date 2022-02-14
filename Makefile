build:
	cargo build-bpf

analyze:
	soteria -c -analyzeAll .

deploy:
	solana program deploy ./target/deploy/strike_wallet.so

test:
	# Note that test-bpf builds then tests. No need to do `make build`
	# beforehand.  To target a specific module, do `make test -e
	# tests=<SPECIFIC_MODULE_NAME>`.
	cargo test-bpf ${tests} -- --nocapture

deploy_and_test: build deploy test

clean:
	rm -r target
