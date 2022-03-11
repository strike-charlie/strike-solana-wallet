// #![cfg(feature = "test-bpf")]

mod common;

pub use common::{instructions::*, utils::*};
use solana_program::{
    instruction::{InstructionError, InstructionError::Custom},
    pubkey::Pubkey,
    system_instruction,
};
use solana_program_test::tokio;
use solana_sdk::{
    program_pack::Pack,
    signature::{Keypair, Signer as SdkSigner},
    transaction::{Transaction, TransactionError},
};
use std::borrow::BorrowMut;
use strike_wallet::{
    error::WalletError,
    model::{
        balance_account::{BalanceAccount, BalanceAccountGuidHash},
        multisig_op::ApprovalDisposition,
        multisig_op::MultisigOp,
    },
};

/// Keypairs and other data generated and used by test setup logic.
struct Fixtures {
    assistant_keypair: Keypair,
    wallet_keypair: Keypair,
    signer_keypairs: Vec<Keypair>,
    balance_account_guid_hashes: Vec<BalanceAccountGuidHash>,
    token_mint_address: Pubkey,
    payer_balance_account_guid_hash: BalanceAccountGuidHash,
    balance_account_addresses: Vec<Pubkey>,
    associated_token_addresses: Vec<Pubkey>,
}

/// Perform a full MultisigOp to create an associated token account for each of
/// the given BalanceAccounts, effectively enabling the SPL token for it.
async fn enable_spl_token(
    context: &mut TestContext,
    wallet_keypair: &Keypair,
    assistant_keypair: &Keypair,
    approver_keypairs: &Vec<Keypair>,
    token_mint_address: &Pubkey,
    balance_account_addresses: &Vec<Pubkey>,
    associated_token_addresses: &Vec<Pubkey>,
    payer_balance_account_guid_hash: &BalanceAccountGuidHash,
    balance_account_guid_hashes: &Vec<BalanceAccountGuidHash>,
    finalize: bool,
    expected_init_error: Option<InstructionError>,
    expected_finalize_error: Option<InstructionError>,
) -> Keypair {
    let rent = context.banks_client.get_rent().await.unwrap();
    let multisig_account_rent = rent.minimum_balance(MultisigOp::LEN);
    let multisig_op_keypair = Keypair::new();

    // initiate the MultisigOp and approve:
    let tx_init = Transaction::new_signed_with_payer(
        &[
            system_instruction::create_account(
                &context.payer.pubkey(),
                &multisig_op_keypair.pubkey(),
                multisig_account_rent,
                MultisigOp::LEN as u64,
                &context.program_id,
            ),
            common::instructions::init_balance_account_enable_spl_token(
                &context.program_id,
                &wallet_keypair.pubkey(),
                &multisig_op_keypair.pubkey(),
                &assistant_keypair.pubkey(),
                token_mint_address,
                associated_token_addresses,
                &payer_balance_account_guid_hash,
                &balance_account_guid_hashes,
            ),
        ],
        Some(&context.payer.pubkey()),
        &[&context.payer, &multisig_op_keypair, &assistant_keypair],
        context.recent_blockhash,
    );

    // ensure the expected error is returned by init, if any
    match expected_init_error {
        None => context
            .banks_client
            .process_transaction(tx_init)
            .await
            .unwrap(),
        Some(error) => {
            assert_eq!(
                context
                    .banks_client
                    .process_transaction(tx_init)
                    .await
                    .unwrap_err()
                    .unwrap(),
                TransactionError::InstructionError(1, error),
            );
            return multisig_op_keypair;
        }
    }

    approve_or_deny_1_of_2_multisig_op(
        context.banks_client.borrow_mut(),
        &context.program_id,
        &multisig_op_keypair.pubkey(),
        &approver_keypairs[0],
        &context.payer,
        &approver_keypairs[1].pubkey(),
        context.recent_blockhash,
        ApprovalDisposition::APPROVE,
    )
    .await;

    // finalize MultisigOp, triggering creation of AT accounts.
    if finalize {
        finalize_enable_spl_token(
            context,
            wallet_keypair,
            &multisig_op_keypair,
            token_mint_address,
            balance_account_addresses,
            associated_token_addresses,
            payer_balance_account_guid_hash,
            balance_account_guid_hashes,
            true, // add funds to payer account
            expected_finalize_error,
        )
        .await;
    }
    multisig_op_keypair
}

/// Transfer lamports from a source wallet (keypair) to a destination account
/// address. If the `from_wallet` arg is None, the function defaults to
/// using `context.payer`'.
pub async fn transfer_lamports(
    context: &mut TestContext,
    from_wallet: Option<&Keypair>,
    to_address: &Pubkey,
    lamports: u64,
) {
    let payer = match from_wallet {
        Some(keypair) => keypair,
        None => &context.payer,
    };
    context
        .banks_client
        .process_transaction(Transaction::new_signed_with_payer(
            &[system_instruction::transfer(
                &payer.pubkey(),
                to_address,
                lamports,
            )],
            Some(&payer.pubkey()),
            &[payer],
            context.recent_blockhash,
        ))
        .await
        .unwrap();
}

/// Perform finalize. The `enable_spl_token` fn does this automatically if its
/// `finalize` param is set. Otherwise, this can be used to when specifically
/// targetting the finalize method for tests.
pub async fn finalize_enable_spl_token(
    context: &mut TestContext,
    wallet_keypair: &Keypair,
    multisig_op_keypair: &Keypair,
    token_mint_address: &Pubkey,
    balance_account_addresses: &Vec<Pubkey>,
    associated_token_addresses: &Vec<Pubkey>,
    payer_balance_account_guid_hash: &BalanceAccountGuidHash,
    balance_account_guid_hashes: &Vec<BalanceAccountGuidHash>,
    add_funds_to_payer_account: bool,
    expected_error: Option<InstructionError>,
) {
    if add_funds_to_payer_account {
        // calculate cumulative rent for all associated token accounts that we
        // are about to create in finalize...
        let (payer_pda, _) =
            BalanceAccount::find_address(payer_balance_account_guid_hash, &context.program_id);
        let cumulative_rent_required = balance_account_guid_hashes.len() as u64
            * context.rent.minimum_balance(spl_token::state::Account::LEN);
        // now add funds to payer account
        transfer_lamports(context, None, &payer_pda, cumulative_rent_required).await;
    }

    let tx_finalize = Transaction::new_signed_with_payer(
        &[
            common::instructions::finalize_balance_account_enable_spl_token(
                &context.program_id,
                &wallet_keypair.pubkey(),
                &multisig_op_keypair.pubkey(),
                &context.payer.pubkey(),
                token_mint_address,
                &balance_account_addresses[0], // the payer BalanceAccount
                balance_account_addresses,
                associated_token_addresses,
                payer_balance_account_guid_hash,
                balance_account_guid_hashes,
            ),
        ],
        Some(&context.payer.pubkey()),
        &[&context.payer],
        context.recent_blockhash,
    );

    // ensure the expected error is returned by finalize, if any
    match expected_error {
        None => context
            .banks_client
            .process_transaction(tx_finalize)
            .await
            .unwrap(),
        Some(error) => {
            assert_eq!(
                context
                    .banks_client
                    .process_transaction(tx_finalize)
                    .await
                    .unwrap_err()
                    .unwrap(),
                TransactionError::InstructionError(0, error),
            );
            return;
        }
    }
}

/// Create a wallet with several BalanceAccounts.
async fn setup_handler_test(context: &mut TestContext, n_balance_accounts: u8) -> Fixtures {
    let assistant_keypair = Keypair::new();
    let wallet_keypair = Keypair::new();
    let signer_keypairs = vec![Keypair::new(), Keypair::new()];

    create_wallet(
        context,
        &wallet_keypair,
        &assistant_keypair,
        &signer_keypairs,
    )
    .await;

    let balance_account_guid_hashes = create_balance_accounts(
        context,
        &wallet_keypair.pubkey(),
        &assistant_keypair,
        &signer_keypairs,
        n_balance_accounts,
        None,
    )
    .await
    .iter()
    .map(|(hash, _)| *hash)
    .collect();

    let token_mint_address = spl_token::native_mint::id();

    let balance_account_addresses =
        find_balance_account_addresses(&balance_account_guid_hashes, &context.program_id);

    let associated_token_addresses =
        get_associated_token_account_addresses(&balance_account_addresses, &token_mint_address);

    let payer_balance_account_guid_hash = balance_account_guid_hashes[0].clone();

    Fixtures {
        assistant_keypair,
        wallet_keypair,
        signer_keypairs,
        balance_account_guid_hashes,
        token_mint_address,
        balance_account_addresses,
        associated_token_addresses,
        payer_balance_account_guid_hash,
    }
}

#[tokio::test]
async fn test_enable_spl_token_happy_path() {
    let mut ctx = common::utils::setup_test(200_000).await;
    let fixtures = setup_handler_test(&mut ctx, 5).await;

    enable_spl_token(
        &mut ctx,
        &fixtures.wallet_keypair,
        &fixtures.assistant_keypair,
        &fixtures.signer_keypairs,
        &fixtures.token_mint_address,
        &fixtures.balance_account_addresses,
        &fixtures.associated_token_addresses,
        &fixtures.payer_balance_account_guid_hash,
        &fixtures.balance_account_guid_hashes,
        true,
        None,
        None,
    )
    .await;
}

#[tokio::test]
async fn test_enable_spl_token_with_bad_payer_balance_account_hash() {
    let mut context = common::utils::setup_test(50_000).await;
    let fixtures = setup_handler_test(&mut context, 2).await;
    let expected_error = Custom(WalletError::BalanceAccountNotFound as u32);

    // send a random account GUID for the payer param
    let invalid_payer_guid_hash = random_balance_account_guid_hash();

    enable_spl_token(
        &mut context,
        &fixtures.wallet_keypair,
        &fixtures.assistant_keypair,
        &fixtures.signer_keypairs,
        &fixtures.token_mint_address,
        &fixtures.balance_account_addresses,
        &fixtures.associated_token_addresses,
        &invalid_payer_guid_hash,
        &fixtures.balance_account_guid_hashes,
        false, // don't finalize
        Some(expected_error),
        None,
    )
    .await;
}

#[tokio::test]
async fn test_enable_spl_token_with_invalid_token_mint() {
    let mut ctx = common::utils::setup_test(50_000).await;
    let fixtures = setup_handler_test(&mut ctx, 2).await;
    let expected_error = Custom(WalletError::AccountNotRecognized as u32);

    // sent different token mint address than what's sent as MultisigOp param
    let invalid_token_mint_address = Pubkey::new_unique();

    enable_spl_token(
        &mut ctx,
        &fixtures.wallet_keypair,
        &fixtures.assistant_keypair,
        &fixtures.signer_keypairs,
        &invalid_token_mint_address,
        &fixtures.balance_account_addresses,
        &fixtures.associated_token_addresses,
        &fixtures.payer_balance_account_guid_hash,
        &fixtures.balance_account_guid_hashes,
        false, // don't finalize
        Some(expected_error),
        None,
    )
    .await;
}

#[tokio::test]
async fn test_enable_spl_token_with_empty_guid_hash_vec() {
    let mut ctx = common::utils::setup_test(50_000).await;
    let fixtures = setup_handler_test(&mut ctx, 1).await;
    let expected_error = InstructionError::NotEnoughAccountKeys;

    // empty out the GUID hash vec.
    let mut invalid_account_guid_hashes = fixtures.balance_account_guid_hashes.clone();
    invalid_account_guid_hashes.pop();

    // send 6 guid hashes, verifying the the limit is 5.
    enable_spl_token(
        &mut ctx,
        &fixtures.wallet_keypair,
        &fixtures.assistant_keypair,
        &fixtures.signer_keypairs,
        &fixtures.token_mint_address,
        &fixtures.balance_account_addresses,
        &fixtures.associated_token_addresses,
        &fixtures.payer_balance_account_guid_hash,
        &invalid_account_guid_hashes,
        false, // don't finalize
        Some(expected_error),
        None,
    )
    .await;
}

#[tokio::test]
async fn test_enable_spl_token_with_too_many_guid_hashes() {
    let mut ctx = common::utils::setup_test(150_000).await;
    let fixtures = setup_handler_test(&mut ctx, 6).await;
    let expected_error = InstructionError::InvalidArgument;

    // send 1 too many guids hashes.
    // max value defined in balance_account_enable_spl_token_handler.
    enable_spl_token(
        &mut ctx,
        &fixtures.wallet_keypair,
        &fixtures.assistant_keypair,
        &fixtures.signer_keypairs,
        &fixtures.token_mint_address,
        &fixtures.balance_account_addresses,
        &fixtures.associated_token_addresses,
        &fixtures.payer_balance_account_guid_hash,
        &fixtures.balance_account_guid_hashes,
        false, // don't finalize
        Some(expected_error),
        None,
    )
    .await;
}

#[tokio::test]
async fn test_enable_spl_token_with_invalid_guid_hash() {
    let mut ctx = common::utils::setup_test(50_000).await;
    let fixtures = setup_handler_test(&mut ctx, 2).await;
    let expected_error = Custom(WalletError::BalanceAccountNotFound as u32);

    // alter one of the guid hashes
    let mut invalid_account_guid_hashes = fixtures.balance_account_guid_hashes.clone();
    invalid_account_guid_hashes.pop();
    invalid_account_guid_hashes.push(random_balance_account_guid_hash());

    enable_spl_token(
        &mut ctx,
        &fixtures.wallet_keypair,
        &fixtures.assistant_keypair,
        &fixtures.signer_keypairs,
        &fixtures.token_mint_address,
        &fixtures.balance_account_addresses,
        &fixtures.associated_token_addresses,
        &fixtures.payer_balance_account_guid_hash,
        &invalid_account_guid_hashes,
        false, // don't finalize
        Some(expected_error),
        None,
    )
    .await;
}

#[tokio::test]
async fn test_enable_spl_token_with_insufficient_account_keys_in_finalize() {
    let mut ctx = common::utils::setup_test(50_000).await;
    let fixtures = setup_handler_test(&mut ctx, 2).await;
    let expected_error = InstructionError::NotEnoughAccountKeys;

    // remove one of the balance accounts sent in finalize
    let mut insufficient_balance_account_addresses = fixtures.balance_account_addresses.clone();
    insufficient_balance_account_addresses.pop();

    enable_spl_token(
        &mut ctx,
        &fixtures.wallet_keypair,
        &fixtures.assistant_keypair,
        &fixtures.signer_keypairs,
        &fixtures.token_mint_address,
        &insufficient_balance_account_addresses,
        &fixtures.associated_token_addresses,
        &fixtures.payer_balance_account_guid_hash,
        &fixtures.balance_account_guid_hashes,
        true, // finalize
        None,
        Some(expected_error),
    )
    .await;
}

#[tokio::test]
async fn test_enable_spl_token_with_invalid_balance_account_address_in_finalize() {
    let mut ctx = common::utils::setup_test(100_000).await;
    let fixtures = setup_handler_test(&mut ctx, 2).await;
    let expected_error = InstructionError::InsufficientFunds;

    let multisig_op_keypair = enable_spl_token(
        &mut ctx,
        &fixtures.wallet_keypair,
        &fixtures.assistant_keypair,
        &fixtures.signer_keypairs,
        &fixtures.token_mint_address,
        &fixtures.balance_account_addresses,
        &fixtures.associated_token_addresses,
        &fixtures.payer_balance_account_guid_hash,
        &fixtures.balance_account_guid_hashes,
        false, // do not finalize here
        None,
        None,
    )
    .await;

    finalize_enable_spl_token(
        &mut ctx,
        &fixtures.wallet_keypair,
        &multisig_op_keypair,
        &fixtures.token_mint_address,
        &fixtures.balance_account_addresses,
        &fixtures.associated_token_addresses,
        &fixtures.payer_balance_account_guid_hash,
        &fixtures.balance_account_guid_hashes,
        false, // do not fund the payer account to trigger expected error
        Some(expected_error),
    )
    .await;
}
