#![cfg(feature = "test-bpf")]

mod common;

use std::borrow::BorrowMut;

use crate::common::utils;
use common::instructions::{
    finalize_dapp_transaction, init_dapp_transaction, init_transfer, set_approval_disposition,
};
use solana_program::instruction::InstructionError::Custom;
use solana_program::program_pack::Pack;
use solana_program::{system_instruction, system_program};
use solana_program_test::tokio;
use solana_sdk::account::ReadableAccount;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer as SdkSigner;
use solana_sdk::transaction::{Transaction, TransactionError};
use strike_wallet::error::WalletError;
use strike_wallet::model::multisig_op::{ApprovalDisposition, MultisigOp};

#[tokio::test]
async fn test_dapp_transaction() {
    let (mut context, balance_account) =
        utils::setup_balance_account_tests_and_finalize(Some(100000)).await;

    let rent = context.banks_client.get_rent().await.unwrap();
    let multisig_account_rent = rent.minimum_balance(MultisigOp::LEN);
    let multisig_op_account = Keypair::new();

    let inner_multisig_op_account = Keypair::new();

    let inner_instructions = vec![
        system_instruction::create_account(
            &context.payer.pubkey(),
            &inner_multisig_op_account.pubkey(),
            multisig_account_rent,
            MultisigOp::LEN as u64,
            &context.program_id,
        ),
        init_transfer(
            &context.program_id,
            &context.wallet_account.pubkey(),
            &inner_multisig_op_account.pubkey(),
            &context.assistant_account.pubkey(),
            &balance_account,
            &context.destination.pubkey(),
            context.balance_account_guid_hash,
            123,
            context.destination_name_hash,
            &system_program::id(),
            &context.payer.pubkey(),
        ),
    ];

    context
        .banks_client
        .process_transaction(Transaction::new_signed_with_payer(
            &[
                system_instruction::create_account(
                    &context.payer.pubkey(),
                    &multisig_op_account.pubkey(),
                    multisig_account_rent,
                    MultisigOp::LEN as u64,
                    &context.program_id,
                ),
                init_dapp_transaction(
                    &context.program_id,
                    &context.wallet_account.pubkey(),
                    &multisig_op_account.pubkey(),
                    &context.assistant_account.pubkey(),
                    &context.balance_account_guid_hash,
                    inner_instructions.clone(),
                ),
            ],
            Some(&context.payer.pubkey()),
            &[
                &context.payer,
                &multisig_op_account,
                &context.assistant_account,
            ],
            context.recent_blockhash,
        ))
        .await
        .unwrap();

    // attempting to finalize before approval should result in a transaction simulation
    assert_eq!(
        context
            .banks_client
            .process_transaction(Transaction::new_signed_with_payer(
                &[finalize_dapp_transaction(
                    &context.program_id,
                    &context.wallet_account.pubkey(),
                    &multisig_op_account.pubkey(),
                    &balance_account,
                    &context.payer.pubkey(),
                    &context.balance_account_guid_hash,
                    &inner_instructions,
                )],
                Some(&context.payer.pubkey()),
                &[
                    &context.payer,
                    &context.assistant_account,
                    &inner_multisig_op_account,
                ],
                context.recent_blockhash,
            ))
            .await
            .unwrap_err()
            .unwrap(),
        TransactionError::InstructionError(0, Custom(WalletError::SimulationFinished as u32)),
    );

    let params_hash = utils::get_operation_hash(
        context.banks_client.borrow_mut(),
        multisig_op_account.pubkey(),
    )
    .await;
    let approver = &context.approvers[0];
    let approve_transaction = Transaction::new_signed_with_payer(
        &[set_approval_disposition(
            &context.program_id,
            &multisig_op_account.pubkey(),
            &approver.pubkey(),
            ApprovalDisposition::APPROVE,
            params_hash,
        )],
        Some(&context.payer.pubkey()),
        &[&context.payer, approver],
        context.recent_blockhash,
    );
    context
        .banks_client
        .process_transaction(approve_transaction)
        .await
        .unwrap();

    let blockhash = context.banks_client.get_recent_blockhash().await.unwrap();

    context
        .banks_client
        .process_transaction(Transaction::new_signed_with_payer(
            &[finalize_dapp_transaction(
                &context.program_id,
                &context.wallet_account.pubkey(),
                &multisig_op_account.pubkey(),
                &balance_account,
                &context.payer.pubkey(),
                &context.balance_account_guid_hash,
                &inner_instructions,
            )],
            Some(&context.payer.pubkey()),
            &[
                &context.payer,
                &context.assistant_account,
                &inner_multisig_op_account,
            ],
            blockhash,
        ))
        .await
        .unwrap();

    let multisig_op = MultisigOp::unpack_from_slice(
        context
            .banks_client
            .get_account(inner_multisig_op_account.pubkey())
            .await
            .unwrap()
            .unwrap()
            .data(),
    )
    .unwrap();
    assert!(multisig_op.is_initialized);
}

#[tokio::test]
async fn test_dapp_transaction_with_spl_transfers() {
    let (mut context, balance_account) =
        utils::setup_balance_account_tests_and_finalize(Some(200000)).await;

    let rent = context.banks_client.get_rent().await.unwrap();
    let multisig_account_rent = rent.minimum_balance(MultisigOp::LEN);
    let multisig_op_account = Keypair::new();
    let mint_account_rent = rent.minimum_balance(spl_token::state::Mint::LEN);
    let mint = Keypair::new();
    let mint_authority = Keypair::new();
    let source_token_address = spl_associated_token_account::get_associated_token_address(
        &balance_account,
        &mint.pubkey(),
    );

    let inner_instructions = vec![
        system_instruction::create_account(
            &context.payer.pubkey(),
            &mint.pubkey(),
            mint_account_rent,
            spl_token::state::Mint::LEN as u64,
            &spl_token::id(),
        ),
        system_instruction::create_account(
            &context.payer.pubkey(),
            &mint_authority.pubkey(),
            0,
            0,
            &system_program::id(),
        ),
        spl_token::instruction::initialize_mint(
            &spl_token::id(),
            &mint.pubkey(),
            &mint_authority.pubkey(),
            Some(&mint_authority.pubkey()),
            6,
        )
        .unwrap(),
        spl_associated_token_account::create_associated_token_account(
            &context.payer.pubkey(),
            &balance_account,
            &mint.pubkey(),
        ),
        spl_token::instruction::mint_to(
            &spl_token::id(),
            &mint.pubkey(),
            &source_token_address,
            &mint_authority.pubkey(),
            &[],
            1000,
        )
        .unwrap(),
    ];

    context
        .banks_client
        .process_transaction(Transaction::new_signed_with_payer(
            &[
                system_instruction::create_account(
                    &context.payer.pubkey(),
                    &multisig_op_account.pubkey(),
                    multisig_account_rent,
                    MultisigOp::LEN as u64,
                    &context.program_id,
                ),
                init_dapp_transaction(
                    &context.program_id,
                    &context.wallet_account.pubkey(),
                    &multisig_op_account.pubkey(),
                    &context.assistant_account.pubkey(),
                    &context.balance_account_guid_hash,
                    inner_instructions.clone(),
                ),
            ],
            Some(&context.payer.pubkey()),
            &[
                &context.payer,
                &multisig_op_account,
                &context.assistant_account,
            ],
            context.recent_blockhash,
        ))
        .await
        .unwrap();

    // attempting to finalize before approval should result in a transaction simulation
    assert_eq!(
        context
            .banks_client
            .process_transaction(Transaction::new_signed_with_payer(
                &[finalize_dapp_transaction(
                    &context.program_id,
                    &context.wallet_account.pubkey(),
                    &multisig_op_account.pubkey(),
                    &balance_account,
                    &context.payer.pubkey(),
                    &context.balance_account_guid_hash,
                    &inner_instructions,
                )],
                Some(&context.payer.pubkey()),
                &[&context.payer, &mint, &mint_authority],
                context.recent_blockhash,
            ))
            .await
            .unwrap_err()
            .unwrap(),
        TransactionError::InstructionError(0, Custom(WalletError::SimulationFinished as u32)),
    );
}
