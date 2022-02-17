#![cfg(feature = "test-bpf")]

mod common;

pub use common::instructions::*;
pub use common::utils::*;

use std::borrow::BorrowMut;

use solana_program::hash::Hash;
use solana_program::instruction::InstructionError::Custom;
use solana_program::system_program;
use solana_sdk::transaction::TransactionError;

use common::instructions::finalize_transfer;
use strike_wallet::error::WalletError;
use strike_wallet::model::address_book::AddressBookEntryNameHash;
use strike_wallet::model::multisig_op::{
    ApprovalDisposition, BooleanSetting, OperationDisposition,
};
use strike_wallet::utils::SlotId;
use {
    solana_program::system_instruction,
    solana_program_test::tokio,
    solana_sdk::{signature::Signer as SdkSigner, transaction::Transaction},
};

#[tokio::test]
async fn test_transfer_sol() {
    let (mut context, balance_account) = setup_balance_account_tests_and_finalize(None).await;
    let (multisig_op_account, result) =
        setup_transfer_test(context.borrow_mut(), &balance_account, None, None).await;
    result.unwrap();

    approve_or_deny_n_of_n_multisig_op(
        context.banks_client.borrow_mut(),
        &context.program_id,
        &multisig_op_account.pubkey(),
        vec![&context.approvers[0], &context.approvers[1]],
        &context.payer,
        context.recent_blockhash,
        ApprovalDisposition::APPROVE,
        OperationDisposition::APPROVED,
    )
    .await;

    // transfer enough balance from fee payer to source account
    context
        .banks_client
        .process_transaction(Transaction::new_signed_with_payer(
            &[system_instruction::transfer(
                &context.payer.pubkey(),
                &balance_account,
                1000,
            )],
            Some(&context.payer.pubkey()),
            &[&context.payer],
            context.recent_blockhash,
        ))
        .await
        .unwrap();

    assert_eq!(
        context
            .banks_client
            .get_balance(balance_account)
            .await
            .unwrap(),
        1000
    );
    assert_eq!(
        context
            .banks_client
            .get_balance(context.destination.pubkey())
            .await
            .unwrap(),
        0
    );

    context
        .banks_client
        .process_transaction(Transaction::new_signed_with_payer(
            &[finalize_transfer(
                &context.program_id,
                &multisig_op_account.pubkey(),
                &context.wallet_account.pubkey(),
                &balance_account,
                &context.destination.pubkey(),
                &context.payer.pubkey(),
                context.balance_account_guid_hash,
                123,
                &system_program::id(),
                None,
            )],
            Some(&context.payer.pubkey()),
            &[&context.payer],
            context.recent_blockhash,
        ))
        .await
        .unwrap();

    assert_eq!(
        context
            .banks_client
            .get_balance(balance_account)
            .await
            .unwrap(),
        1000 - 123
    );
    assert_eq!(
        context
            .banks_client
            .get_balance(context.destination.pubkey())
            .await
            .unwrap(),
        123
    );
}

#[tokio::test]
async fn test_transfer_sol_denied() {
    let (mut context, balance_account) = setup_balance_account_tests_and_finalize(None).await;
    let (multisig_op_account, result) =
        setup_transfer_test(context.borrow_mut(), &balance_account, None, None).await;
    result.unwrap();

    approve_or_deny_n_of_n_multisig_op(
        context.banks_client.borrow_mut(),
        &context.program_id,
        &multisig_op_account.pubkey(),
        vec![&context.approvers[0], &context.approvers[1]],
        &context.payer,
        context.recent_blockhash,
        ApprovalDisposition::DENY,
        OperationDisposition::DENIED,
    )
    .await;

    // transfer enough balance from fee payer to source account
    context
        .banks_client
        .process_transaction(Transaction::new_signed_with_payer(
            &[system_instruction::transfer(
                &context.payer.pubkey(),
                &balance_account,
                1000,
            )],
            Some(&context.payer.pubkey()),
            &[&context.payer],
            context.recent_blockhash,
        ))
        .await
        .unwrap();

    assert_eq!(
        context
            .banks_client
            .get_balance(balance_account)
            .await
            .unwrap(),
        1000
    );
    assert_eq!(
        context
            .banks_client
            .get_balance(context.destination.pubkey())
            .await
            .unwrap(),
        0
    );

    context
        .banks_client
        .process_transaction(Transaction::new_signed_with_payer(
            &[finalize_transfer(
                &context.program_id,
                &multisig_op_account.pubkey(),
                &context.wallet_account.pubkey(),
                &balance_account,
                &context.destination.pubkey(),
                &context.payer.pubkey(),
                context.balance_account_guid_hash,
                123,
                &system_program::id(),
                None,
            )],
            Some(&context.payer.pubkey()),
            &[&context.payer],
            context.recent_blockhash,
        ))
        .await
        .unwrap();

    // balances should all be the same
    assert_eq!(
        context
            .banks_client
            .get_balance(balance_account)
            .await
            .unwrap(),
        1000
    );
    assert_eq!(
        context
            .banks_client
            .get_balance(context.destination.pubkey())
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn test_transfer_wrong_destination_name_hash() {
    let (mut context, balance_account) = setup_balance_account_tests_and_finalize(None).await;

    account_settings_update(&mut context, Some(BooleanSetting::On), None, None).await;
    let destination_to_add = context.allowed_destination;
    modify_whitelist(
        &mut context,
        vec![(SlotId::new(0), destination_to_add)],
        vec![],
        None,
    )
    .await;

    context.destination_name_hash = AddressBookEntryNameHash::zero();

    let (_, result) = setup_transfer_test(context.borrow_mut(), &balance_account, None, None).await;
    assert_eq!(
        result.unwrap_err().unwrap(),
        TransactionError::InstructionError(1, Custom(WalletError::DestinationNotAllowed as u32)),
    )
}

#[tokio::test]
async fn test_transfer_requires_multisig() {
    let (mut context, balance_account) = setup_balance_account_tests_and_finalize(None).await;
    let (multisig_op_account, result) =
        setup_transfer_test(context.borrow_mut(), &balance_account, None, None).await;
    result.unwrap();

    approve_or_deny_1_of_2_multisig_op(
        context.banks_client.borrow_mut(),
        &context.program_id,
        &multisig_op_account.pubkey(),
        &context.approvers[0],
        &context.payer,
        &context.approvers[1].pubkey(),
        context.recent_blockhash,
        ApprovalDisposition::APPROVE,
    )
    .await;

    assert_eq!(
        context
            .banks_client
            .process_transaction(Transaction::new_signed_with_payer(
                &[finalize_transfer(
                    &context.program_id,
                    &multisig_op_account.pubkey(),
                    &context.wallet_account.pubkey(),
                    &balance_account,
                    &context.destination.pubkey(),
                    &context.payer.pubkey(),
                    context.balance_account_guid_hash,
                    123,
                    &system_program::id(),
                    None,
                )],
                Some(&context.payer.pubkey()),
                &[&context.payer],
                context.recent_blockhash,
            ))
            .await
            .unwrap_err()
            .unwrap(),
        TransactionError::InstructionError(
            0,
            Custom(WalletError::TransferDispositionNotFinal as u32)
        ),
    );
}

#[tokio::test]
async fn test_approval_fails_if_incorrect_params_hash() {
    let (mut context, balance_account) = setup_balance_account_tests_and_finalize(None).await;
    let (multisig_op_account, result) =
        setup_transfer_test(context.borrow_mut(), &balance_account, None, None).await;
    result.unwrap();

    assert_eq!(
        context
            .banks_client
            .process_transaction(Transaction::new_signed_with_payer(
                &[set_approval_disposition(
                    &context.program_id,
                    &multisig_op_account.pubkey(),
                    &context.approvers[1].pubkey(),
                    ApprovalDisposition::APPROVE,
                    Hash::new_from_array([0; 32])
                )],
                Some(&context.payer.pubkey()),
                &[&context.payer, &context.approvers[1]],
                context.recent_blockhash,
            ))
            .await
            .unwrap_err()
            .unwrap(),
        TransactionError::InstructionError(0, Custom(WalletError::InvalidSignature as u32)),
    );
}

#[tokio::test]
async fn test_transfer_insufficient_balance() {
    let (mut context, balance_account) = setup_balance_account_tests_and_finalize(None).await;
    let (multisig_op_account, result) =
        setup_transfer_test(context.borrow_mut(), &balance_account, None, None).await;
    result.unwrap();

    approve_or_deny_n_of_n_multisig_op(
        context.banks_client.borrow_mut(),
        &context.program_id,
        &multisig_op_account.pubkey(),
        vec![&context.approvers[0], &context.approvers[1]],
        &context.payer,
        context.recent_blockhash,
        ApprovalDisposition::APPROVE,
        OperationDisposition::APPROVED,
    )
    .await;

    assert_eq!(
        context
            .banks_client
            .process_transaction(Transaction::new_signed_with_payer(
                &[finalize_transfer(
                    &context.program_id,
                    &multisig_op_account.pubkey(),
                    &context.wallet_account.pubkey(),
                    &balance_account,
                    &context.destination.pubkey(),
                    &context.payer.pubkey(),
                    context.balance_account_guid_hash,
                    123,
                    &system_program::id(),
                    None,
                )],
                Some(&context.payer.pubkey()),
                &[&context.payer],
                context.recent_blockhash,
            ))
            .await
            .unwrap_err()
            .unwrap(),
        TransactionError::InstructionError(0, Custom(WalletError::InsufficientBalance as u32)),
    );
}

#[tokio::test]
async fn test_transfer_unwhitelisted_address() {
    let (mut context, balance_account) = setup_balance_account_tests_and_finalize(None).await;
    account_settings_update(&mut context, Some(BooleanSetting::On), None, None).await;

    let (_, result) = setup_transfer_test(context.borrow_mut(), &balance_account, None, None).await;
    assert_eq!(
        result.unwrap_err().unwrap(),
        TransactionError::InstructionError(1, Custom(WalletError::DestinationNotAllowed as u32)),
    );
}
