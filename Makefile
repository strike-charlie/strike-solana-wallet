# If quiet == true, only display total pass/fail count for each test module.
# Example Usage: `make quiet=true test`.
quiet?=false
ifeq "$(quiet)" "true"
	test-modifiers=--nocapture 2>&1 | grep -P '(test result)|(\s+Running)'
else
	test-modifiers=--nocapture
endif


# Show backtrace for test errors if backtrace true.
# Example Usage: `make backtrace=true test`.
backtrace?=false
ifeq "$(backtrace)" "true"
	rust-backtrace=1
else
	rust-backtrace=0
endif


build: format
	cargo build-bpf

analyze:
	soteria -c -analyzeAll .

deploy:
	solana program deploy ./target/deploy/strike_wallet.so

clean:
	rm -r target

format:
	cargo fmt

test:
	RUST_BACKTRACE=${rust-backtrace} cargo test-bpf ${tests} -- ${test-modifiers}

deploy_and_test: build deploy test

test-balance-account-update:
	RUST_BACKTRACE=${rust-backtrace} cargo test-bpf --test=balance_account_update_tests

test-balance-account-creation:
	RUST_BACKTRACE=${rust-backtrace} cargo test-bpf --test=balance_account_creation_tests

test-balance-account-transfer:
	RUST_BACKTRACE=${rust-backtrace} cargo test-bpf --test=balance_account_transfer_tests

test-balance-account-spl-transfer:
	RUST_BACKTRACE=${rust-backtrace} cargo test-bpf --test=balance_account_spl_transfer_tests

test-balance-account-update-whitelist-status:
	RUST_BACKTRACE=${rust-backtrace} cargo test-bpf --test=balance_account_update_whitelist_status_tests

test-wallet-update-signers:
	RUST_BACKTRACE=${rust-backtrace} cargo test-bpf --test=wallet_update_signers_tests

test-wallet-config-policy-update:
	RUST_BACKTRACE=${rust-backtrace} cargo test-bpf --test=wallet_config_policy_update_tests

test-dapp-transactions:
	RUST_BACKTRACE=${rust-backtrace} cargo test-bpf --test=dapp_transaction_tests

test-dapp-book-update:
	RUST_BACKTRACE=${rust-backtrace} cargo test-bpf --test=dapp_book_update_tests

test-init-wallet:
	RUST_BACKTRACE=${rust-backtrace} cargo test-bpf --test=init_wallet_tests
