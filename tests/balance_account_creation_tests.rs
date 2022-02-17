#![cfg(feature = "test-bpf")]

mod common;

pub use common::instructions::*;
pub use common::utils::*;

use std::borrow::BorrowMut;
use std::time::Duration;

use solana_program::instruction::InstructionError::{Custom, MissingRequiredSignature};
use solana_sdk::transaction::TransactionError;

use crate::common::utils;
use common::instructions::finalize_balance_account_creation;
use std::collections::HashSet;
use strike_wallet::error::WalletError;
use strike_wallet::model::balance_account::BalanceAccountGuidHash;
use strike_wallet::model::multisig_op::ApprovalDisposition;
use strike_wallet::model::wallet::Wallet;
use {
    solana_program_test::tokio,
    solana_sdk::{
        pubkey::Pubkey,
        signature::{Keypair, Signer as SdkSigner},
        transaction::Transaction,
    },
};

#[tokio::test]
async fn test_balance_account_creation() {
    let mut context = setup_balance_account_tests(None, false).await;

    approve_or_deny_1_of_2_multisig_op(
        context.banks_client.borrow_mut(),
        &context.program_id,
        &context.multisig_op_account.pubkey(),
        &context.approvers[0],
        &context.payer,
        &context.approvers[1].pubkey(),
        context.recent_blockhash,
        ApprovalDisposition::APPROVE,
    )
    .await;

    // finalize
    let starting_rent_collector_balance = context
        .banks_client
        .get_balance(context.payer.pubkey())
        .await
        .unwrap();
    let op_account_balance = context
        .banks_client
        .get_balance(context.multisig_op_account.pubkey())
        .await
        .unwrap();
    utils::finalize_balance_account_creation(context.borrow_mut()).await;

    // verify that it was created as expected
    let wallet = get_wallet(&mut context.banks_client, &context.wallet_account.pubkey()).await;

    let balance_account = wallet
        .get_balance_account(&context.balance_account_guid_hash)
        .unwrap();

    assert_eq!(balance_account.guid_hash, context.balance_account_guid_hash);
    assert_eq!(balance_account.name_hash, context.balance_account_name_hash);
    assert_eq!(
        wallet.get_transfer_approvers_keys(balance_account).to_set(),
        HashSet::from([context.approvers[0].pubkey(), context.approvers[1].pubkey()])
    );
    assert_eq!(
        wallet.get_allowed_destinations(balance_account).to_set(),
        HashSet::from([])
    );
    assert_eq!(balance_account.approvals_required_for_transfer, 2);
    assert_eq!(
        balance_account.approval_timeout_for_transfer,
        Duration::from_secs(120)
    );

    // verify the multisig op account is closed
    assert!(context
        .banks_client
        .get_account(context.multisig_op_account.pubkey())
        .await
        .unwrap()
        .is_none());
    // and that the remaining balance went to the rent collector (less the 5000 in signature fees for the finalize)
    let ending_rent_collector_balance = context
        .banks_client
        .get_balance(context.payer.pubkey())
        .await
        .unwrap();
    assert_eq!(
        starting_rent_collector_balance + op_account_balance - 5000,
        ending_rent_collector_balance
    );
}

#[tokio::test]
async fn test_balance_account_creation_fails_if_timeout_invalid() {
    let invalid_timeout_secs = vec![
        Wallet::MIN_APPROVAL_TIMEOUT.as_secs() - 1,
        Wallet::MAX_APPROVAL_TIMEOUT.as_secs() + 1,
    ];
    for secs in invalid_timeout_secs.iter() {
        let invalid_timeout = Duration::from_secs(*secs);
        assert_eq!(
            utils::setup_create_balance_account_failure_tests(
                None,
                1,
                invalid_timeout,
                vec![Pubkey::new_unique()]
            )
            .await,
            TransactionError::InstructionError(
                1,
                Custom(WalletError::InvalidApprovalTimeout as u32)
            ),
        )
    }
}

#[tokio::test]
async fn test_balance_account_creation_fails_if_no_approvers() {
    assert_eq!(
        setup_create_balance_account_failure_tests(None, 1, Duration::from_secs(18000), vec![])
            .await,
        TransactionError::InstructionError(1, Custom(WalletError::InvalidApproverCount as u32))
    )
}

#[tokio::test]
async fn test_balance_account_creation_fails_if_num_approvals_required_not_set() {
    assert_eq!(
        setup_create_balance_account_failure_tests(
            None,
            0,
            Duration::from_secs(18000),
            vec![Pubkey::new_unique()]
        )
        .await,
        TransactionError::InstructionError(1, Custom(WalletError::InvalidApproverCount as u32))
    )
}

#[tokio::test]
async fn test_balance_account_creation_not_signed_by_rent_collector() {
    let mut context = setup_balance_account_tests(None, false).await;

    let rent_collector = Keypair::new();
    let mut instruction = finalize_balance_account_creation(
        &context.program_id,
        &context.wallet_account.pubkey(),
        &context.multisig_op_account.pubkey(),
        &rent_collector.pubkey(),
        context.balance_account_guid_hash,
        context.expected_update,
    );
    instruction.accounts[2].is_signer = false;

    let finalize_transaction = Transaction::new_signed_with_payer(
        &[instruction],
        Some(&context.payer.pubkey()),
        &[&context.payer],
        context.recent_blockhash,
    );
    assert_eq!(
        context
            .banks_client
            .process_transaction(finalize_transaction)
            .await
            .unwrap_err()
            .unwrap(),
        TransactionError::InstructionError(0, MissingRequiredSignature),
    );
}

#[tokio::test]
async fn test_balance_account_creation_incorrect_hash() {
    let mut context = setup_balance_account_tests(None, false).await;

    let wrong_guid_hash = BalanceAccountGuidHash::zero();

    let finalize_transaction_wrong_wallet_guid_hash = Transaction::new_signed_with_payer(
        &[finalize_balance_account_creation(
            &context.program_id,
            &context.wallet_account.pubkey(),
            &context.multisig_op_account.pubkey(),
            &context.payer.pubkey(),
            wrong_guid_hash,
            context.expected_update.clone(),
        )],
        Some(&context.payer.pubkey()),
        &[&context.payer],
        context.recent_blockhash,
    );
    assert_eq!(
        context
            .banks_client
            .process_transaction(finalize_transaction_wrong_wallet_guid_hash)
            .await
            .unwrap_err()
            .unwrap(),
        TransactionError::InstructionError(0, Custom(WalletError::InvalidSignature as u32)),
    );

    let altered_update = context.expected_update.borrow_mut();
    altered_update.approvals_required_for_transfer = 0;

    let finalize_transaction_wrong_update = Transaction::new_signed_with_payer(
        &[finalize_balance_account_creation(
            &context.program_id,
            &context.wallet_account.pubkey(),
            &context.multisig_op_account.pubkey(),
            &context.payer.pubkey(),
            context.balance_account_guid_hash,
            altered_update.clone(),
        )],
        Some(&context.payer.pubkey()),
        &[&context.payer],
        context.recent_blockhash,
    );

    assert_eq!(
        context
            .banks_client
            .process_transaction(finalize_transaction_wrong_update)
            .await
            .unwrap_err()
            .unwrap(),
        TransactionError::InstructionError(0, Custom(WalletError::InvalidSignature as u32)),
    );
}
