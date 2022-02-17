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
	cargo test-bpf -- --nocapture

deploy_and_test: build deploy test

test-balance-account-update:
	RUST_BACKTRACE=1 cargo test-bpf --test=balance_account_update_tests

test-balance-account-creation:
	RUST_BACKTRACE=1 cargo test-bpf --test=balance_account_creation_tests

test-balance-account-transfer:
	RUST_BACKTRACE=1 cargo test-bpf --test=balance_account_transfer_tests

test-balance-account-spl-transfer:
	RUST_BACKTRACE=1 cargo test-bpf --test=balance_account_spl_transfer_tests

test-balance-account-update-whitelist-status:
	RUST_BACKTRACE=1 cargo test-bpf --test=balance_account_update_whitelist_status_tests

test-wallet-update:
	RUST_BACKTRACE=1 cargo test-bpf --test=wallet_update_tests

test-wallet-update-signers:
	RUST_BACKTRACE=1 cargo test-bpf --test=wallet_update_signers_tests

test-wallet-config-policy-update:
	RUST_BACKTRACE=1 cargo test-bpf --test=wallet_config_policy_update_tests

test-dapp-transactions:
	RUST_BACKTRACE=1 cargo test-bpf --test=dapp_transaction_tests

test-dapp-book-update:
	RUST_BACKTRACE=1 cargo test-bpf --test=dapp_book_update_tests