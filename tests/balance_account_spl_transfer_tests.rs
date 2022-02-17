#![cfg(feature = "test-bpf")]

mod common;

pub use common::instructions::*;
pub use common::utils::*;

use std::borrow::BorrowMut;

use solana_program::instruction::InstructionError::Custom;
use solana_sdk::transaction::TransactionError;

use common::instructions::finalize_transfer;
use strike_wallet::error::WalletError;
use strike_wallet::model::multisig_op::{ApprovalDisposition, OperationDisposition};
use {
    solana_program::system_instruction,
    solana_program_test::tokio,
    solana_sdk::{program_pack::Pack, signature::Signer as SdkSigner, transaction::Transaction},
    strike_wallet::model::multisig_op::MultisigOp,
};

#[tokio::test]
async fn test_wrap_unwrap() {
    let (mut context, balance_account) =
        setup_balance_account_tests_and_finalize(Some(60_000)).await;
    let rent = context.banks_client.get_rent().await.unwrap();
    let token_account_rent = rent.minimum_balance(spl_token::state::Account::LEN);
    let multisig_account_rent = rent.minimum_balance(MultisigOp::LEN);
    let wrapped_sol_account = spl_associated_token_account::get_associated_token_address(
        &balance_account,
        &spl_token::native_mint::id(),
    );

    let amount = 123;

    assert_eq!(
        process_wrap(
            &mut context,
            multisig_account_rent,
            balance_account,
            amount,
            token_account_rent,
            wrapped_sol_account,
        )
        .await
        .unwrap_err()
        .unwrap(),
        TransactionError::InstructionError(1, Custom(1)),
    );

    // move enough into balance account to fund wrapped sol token rent, but NOT what we want to transfer
    context
        .banks_client
        .process_transaction(Transaction::new_signed_with_payer(
            &[system_instruction::transfer(
                &context.payer.pubkey(),
                &balance_account,
                token_account_rent,
            )],
            Some(&context.payer.pubkey()),
            &[&context.payer],
            context.recent_blockhash,
        ))
        .await
        .unwrap();

    assert_eq!(
        process_wrap(
            &mut context,
            multisig_account_rent,
            balance_account,
            amount,
            token_account_rent,
            wrapped_sol_account,
        )
        .await
        .unwrap_err()
        .unwrap(),
        TransactionError::InstructionError(0, Custom(WalletError::InsufficientBalance as u32)),
    );

    // balance account should have 0 SOL now
    assert_eq!(
        context
            .banks_client
            .get_balance(balance_account)
            .await
            .unwrap(),
        0
    );

    // move enough into balance account to fund the amount
    context
        .banks_client
        .process_transaction(Transaction::new_signed_with_payer(
            &[system_instruction::transfer(
                &context.payer.pubkey(),
                &balance_account,
                amount,
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
        amount
    );

    process_wrap(
        &mut context,
        multisig_account_rent,
        balance_account,
        amount,
        token_account_rent,
        wrapped_sol_account,
    )
    .await
    .unwrap();

    assert_eq!(
        context
            .banks_client
            .get_balance(wrapped_sol_account)
            .await
            .unwrap(),
        token_account_rent + amount
    );

    assert_eq!(
        context
            .banks_client
            .get_balance(balance_account)
            .await
            .unwrap(),
        0
    );

    let result = process_unwrapping(
        &mut context,
        multisig_account_rent,
        balance_account,
        amount * 2,
    )
    .await;
    assert_eq!(
        result.unwrap_err().unwrap(),
        TransactionError::InstructionError(0, Custom(WalletError::InsufficientBalance as u32)),
    );

    let unwrap_amount = 64;
    process_unwrapping(
        &mut context,
        multisig_account_rent,
        balance_account,
        unwrap_amount,
    )
    .await
    .unwrap();

    assert_eq!(
        context
            .banks_client
            .get_balance(wrapped_sol_account)
            .await
            .unwrap(),
        token_account_rent + amount - unwrap_amount
    );

    assert_eq!(
        get_token_balance(&mut context, &wrapped_sol_account).await,
        amount - unwrap_amount
    );

    assert_eq!(
        context
            .banks_client
            .get_balance(balance_account)
            .await
            .unwrap(),
        unwrap_amount
    );
}

#[tokio::test]
async fn test_transfer_spl_happy() {
    test_transfer_spl(false, true).await
}

#[tokio::test]
async fn test_transfer_spl_no_funds_in_source() {
    test_transfer_spl(false, false).await
}

#[tokio::test]
async fn test_transfer_spl_destination_token_account_exists() {
    test_transfer_spl(true, false).await
}

async fn test_transfer_spl(
    create_destination_token_account: bool,
    fund_source_account_to_pay_for_token: bool,
) {
    let (mut context, balance_account) =
        setup_balance_account_tests_and_finalize(Some(60_000)).await;

    let spl_context = setup_spl_transfer_test(
        &mut context,
        &balance_account,
        fund_source_account_to_pay_for_token,
    )
    .await;

    if create_destination_token_account {
        context
            .banks_client
            .process_transaction(Transaction::new_signed_with_payer(
                &[
                    spl_associated_token_account::create_associated_token_account(
                        &context.payer.pubkey(),
                        &context.destination.pubkey(),
                        &spl_context.mint.pubkey(),
                    ),
                ],
                Some(&context.payer.pubkey()),
                &[&context.payer],
                context.recent_blockhash,
            ))
            .await
            .unwrap();
    }

    let (multisig_op_account, result) = setup_transfer_test(
        context.borrow_mut(),
        &balance_account,
        Some(&spl_context.mint.pubkey()),
        None,
    )
    .await;
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
        get_token_balance(&mut context, &spl_context.source_token_address).await,
        1000
    );
    assert_eq!(
        get_token_balance(&mut context, &spl_context.destination_token_address).await,
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
                &context.allowed_destination.address,
                &context.payer.pubkey(),
                context.balance_account_guid_hash,
                123,
                &spl_context.mint.pubkey(),
                Some(&spl_context.mint_authority.pubkey()),
            )],
            Some(&context.payer.pubkey()),
            &[&context.payer],
            context.recent_blockhash,
        ))
        .await
        .unwrap();

    assert_eq!(
        get_token_balance(&mut context, &spl_context.source_token_address).await,
        1000 - 123
    );
    assert_eq!(
        get_token_balance(&mut context, &spl_context.destination_token_address).await,
        123
    );
}

#[tokio::test]
async fn test_transfer_spl_insufficient_balance() {
    let (mut context, balance_account) =
        setup_balance_account_tests_and_finalize(Some(60_000)).await;
    let spl_context = setup_spl_transfer_test(&mut context, &balance_account, true).await;

    let (multisig_op_account, result) = setup_transfer_test(
        context.borrow_mut(),
        &balance_account,
        Some(&spl_context.mint.pubkey()),
        Some(1230),
    )
    .await;
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
        get_token_balance(&mut context, &spl_context.source_token_address).await,
        1000
    );
    assert_eq!(
        get_token_balance(&mut context, &spl_context.destination_token_address).await,
        0
    );

    assert_eq!(
        context
            .banks_client
            .process_transaction(Transaction::new_signed_with_payer(
                &[finalize_transfer(
                    &context.program_id,
                    &multisig_op_account.pubkey(),
                    &context.wallet_account.pubkey(),
                    &balance_account,
                    &context.allowed_destination.address,
                    &context.payer.pubkey(),
                    context.balance_account_guid_hash,
                    1230,
                    &spl_context.mint.pubkey(),
                    Some(&spl_context.mint_authority.pubkey()),
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

    assert_eq!(
        get_token_balance(&mut context, &spl_context.source_token_address).await,
        1000
    );
    assert_eq!(
        get_token_balance(&mut context, &spl_context.destination_token_address).await,
        0
    );
}
