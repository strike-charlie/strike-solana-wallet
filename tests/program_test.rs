#![cfg(feature = "test-bpf")]
use std::borrow::BorrowMut;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use solana_program::instruction::InstructionError::{
    Custom, InvalidArgument, MissingRequiredSignature,
};
use solana_program::system_program;
use solana_sdk::transaction::TransactionError;

use strike_wallet::error::WalletError;
use strike_wallet::instruction::{
    finalize_config_update, finalize_transfer, finalize_wallet_config_update,
    finalize_wallet_creation, init_wallet_config_update, set_approval_disposition,
    WalletConfigUpdate,
};
use strike_wallet::model::multisig_op::{ApprovalDisposition, ApprovalDispositionRecord,
                                        OperationDisposition};
use strike_wallet::model::wallet_config::{AllowedDestination, WalletConfig};
use {
    solana_program::system_instruction,
    solana_program_test::{processor, tokio, ProgramTest},
    solana_sdk::{
        account::ReadableAccount,
        program_pack::Pack,
        pubkey::Pubkey,
        signature::{Keypair, Signer},
        transaction::Transaction,
    },
    strike_wallet::{
        model::{
            multisig_op::{MultisigOp, MultisigOpParams},
            program_config::ProgramConfig,
        },
        processor::Processor,
    },
};

mod utils;

async fn init_program_test(
    approvals_required_for_config: Option<u8>,
    config_approvers: Option<Vec<Pubkey>>,
    approval_timeout_for_config: Option<Duration>,
) {
    let program_owner = Keypair::new();
    let mut pt = ProgramTest::new(
        "strike_wallet",
        program_owner.pubkey(),
        processor!(Processor::process),
    );
    pt.set_bpf_compute_max_units(5_000);
    let (mut banks_client, payer, recent_blockhash) = pt.start().await;
    let program_config_account = Keypair::new();
    let assistant_account = Keypair::new();

    utils::init_program(
        &mut banks_client,
        &payer,
        recent_blockhash,
        &program_owner,
        &program_config_account,
        &assistant_account,
        approvals_required_for_config,
        config_approvers.clone(),
        approval_timeout_for_config
    )
    .await
    .unwrap();

    let config = ProgramConfig::unpack_from_slice(
        banks_client
            .get_account(program_config_account.pubkey())
            .await
            .unwrap()
            .unwrap()
            .data(),
    )
    .unwrap();
    assert!(config.is_initialized);
    assert_eq!(
        config.approvals_required_for_config,
        approvals_required_for_config.unwrap_or(0)
    );
    assert_eq!(
        config.config_approvers,
        config_approvers.unwrap_or(Vec::new())
    );
}

#[tokio::test]
async fn init_program_with_approvers() {
    init_program_test(
        Some(2),
        Some(vec![
            Pubkey::new_unique(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        ]),
        Some(Duration::from_secs(3600))
    )
    .await;
}

#[tokio::test]
async fn config_update() {
    let start = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    let mut context = utils::setup_program_config_update_test().await;

    // verify the multisig op account data
    let multisig_op = MultisigOp::unpack_from_slice(
        context
            .banks_client
            .get_account(context.multisig_op_account.pubkey())
            .await
            .unwrap()
            .unwrap()
            .data(),
    )
    .unwrap();
    assert!(multisig_op.is_initialized);
    assert_eq!(
        multisig_op.disposition_records,
        vec![
            ApprovalDispositionRecord {
                approver: context.approvers[0].pubkey(),
                disposition: ApprovalDisposition::NONE,
            },
            ApprovalDispositionRecord {
                approver: context.approvers[1].pubkey(),
                disposition: ApprovalDisposition::NONE,
            },
        ]
    );
    assert_eq!(multisig_op.dispositions_required, 1);
    assert_eq!(multisig_op.operation_disposition, OperationDisposition::NONE);
    assert_eq!(multisig_op.started_at, start);
    assert_eq!(multisig_op.expires_at, start + 3600);

    assert_eq!(
        multisig_op.params_hash,
        MultisigOpParams::UpdateProgramConfig {
            program_config_address: context.program_config_account.pubkey(),
            config_update: context.expected_config_update.clone(),
        }
        .hash()
    );

    utils::approve_or_deny_1_of_2_multisig_op(
        context.banks_client.borrow_mut(),
        &context.program_owner.pubkey(),
        &context.multisig_op_account.pubkey(),
        &context.approvers[0],
        &context.payer,
        &context.approvers[1].pubkey(),
        context.recent_blockhash,
        ApprovalDisposition::APPROVE
    )
    .await;

    // verify the multisig op account data
    let multisig_op = MultisigOp::unpack_from_slice(
        context
            .banks_client
            .get_account(context.multisig_op_account.pubkey())
            .await
            .unwrap()
            .unwrap()
            .data(),
    )
    .unwrap();
    assert_eq!(multisig_op.operation_disposition, OperationDisposition::APPROVED);

    // finalize the multisig op
    let finalize_transaction = Transaction::new_signed_with_payer(
        &[finalize_config_update(
            &context.program_owner.pubkey(),
            &context.program_config_account.pubkey(),
            &context.multisig_op_account.pubkey(),
            &context.payer.pubkey(),
            context.expected_config_update.clone(),
        )],
        Some(&context.payer.pubkey()),
        &[&context.payer],
        context.recent_blockhash,
    );
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
    context
        .banks_client
        .process_transaction(finalize_transaction)
        .await
        .unwrap();

    // verify the config has been updated
    let config = ProgramConfig::unpack_from_slice(
        context
            .banks_client
            .get_account(context.program_config_account.pubkey())
            .await
            .unwrap()
            .unwrap()
            .data(),
    )
    .unwrap();
    assert_eq!(config.approvals_required_for_config, 2);
    assert_eq!(config.approval_timeout_for_config, Duration::from_secs(7200));
    assert_eq!(
        config.config_approvers,
        vec!(context.approvers[1].pubkey(), context.approvers[2].pubkey())
    );

    // verify the multisig op account is closed
    assert!(context
        .banks_client
        .get_account(context.multisig_op_account.pubkey())
        .await
        .unwrap()
        .is_none());
    // and that the remaining balance went to the rent collector (less the 5000 in fees for the finalize)
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
async fn config_update_is_denied() {
    let start = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    let mut context = utils::setup_program_config_update_test().await;

    utils::approve_or_deny_1_of_2_multisig_op(
        context.banks_client.borrow_mut(),
        &context.program_owner.pubkey(),
        &context.multisig_op_account.pubkey(),
        &context.approvers[0],
        &context.payer,
        &context.approvers[1].pubkey(),
        context.recent_blockhash,
        ApprovalDisposition::DENY
    ).await;

    // verify the multisig op account data
    let multisig_op = MultisigOp::unpack_from_slice(
        context
            .banks_client
            .get_account(context.multisig_op_account.pubkey())
            .await
            .unwrap()
            .unwrap()
            .data(),
    ).unwrap();
    assert_eq!(multisig_op.operation_disposition, OperationDisposition::DENIED);
    assert_eq!(multisig_op.started_at, start);
    assert_eq!(multisig_op.expires_at, start + 3600);

    // finalize the multisig op
    let finalize_transaction = Transaction::new_signed_with_payer(
        &[finalize_config_update(
            &context.program_owner.pubkey(),
            &context.program_config_account.pubkey(),
            &context.multisig_op_account.pubkey(),
            &context.payer.pubkey(),
            context.expected_config_update.clone(),
        )],
        Some(&context.payer.pubkey()),
        &[&context.payer],
        context.recent_blockhash,
    );
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
    context
        .banks_client
        .process_transaction(finalize_transaction)
        .await
        .unwrap();

    // verify the config has not been updated
    let config = ProgramConfig::unpack_from_slice(
        context
            .banks_client
            .get_account(context.program_config_account.pubkey())
            .await
            .unwrap()
            .unwrap()
            .data(),
    )
        .unwrap();
    assert_eq!(config.approvals_required_for_config, 1);
    assert_eq!(config.approval_timeout_for_config, Duration::from_secs(3600));
    assert_eq!(
        config.config_approvers,
        vec!(context.approvers[0].pubkey(), context.approvers[1].pubkey())
    );

    // verify the multisig op account is closed
    assert!(context
        .banks_client
        .get_account(context.multisig_op_account.pubkey())
        .await
        .unwrap()
        .is_none());
    // and that the remaining balance went to the rent collector (less the 5000 in fees for the finalize)
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
async fn config_update_finalize_fails() {
    let mut context = utils::setup_program_config_update_test().await;

    // attempt to finalize the multisig op
    let finalize_transaction = Transaction::new_signed_with_payer(
        &[finalize_config_update(
            &context.program_owner.pubkey(),
            &context.program_config_account.pubkey(),
            &context.multisig_op_account.pubkey(),
            &context.payer.pubkey(),
            context.expected_config_update.clone(),
        )],
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
        TransactionError::InstructionError(
            0,
            Custom(WalletError::TransferDispositionNotFinal as u32)
        ),
    );
}

#[tokio::test]
async fn config_update_invalid_approval() {
    let mut context = utils::setup_program_config_update_test().await;

    // attempt to approve the config change with an invalid approver
    let approve_transaction = Transaction::new_signed_with_payer(
        &[set_approval_disposition(
            &context.program_owner.pubkey(),
            &context.multisig_op_account.pubkey(),
            &context.approvers[2].pubkey(),
            ApprovalDisposition::APPROVE,
            context.params_hash
        )],
        Some(&context.payer.pubkey()),
        &[&context.payer, &context.approvers[2]],
        context.recent_blockhash,
    );
    assert_eq!(
        context
            .banks_client
            .process_transaction(approve_transaction)
            .await
            .unwrap_err()
            .unwrap(),
        TransactionError::InstructionError(0, Custom(WalletError::InvalidApprover as u32)),
    );
}

#[tokio::test]
async fn config_update_not_signed_by_rent_collector() {
    let mut context = utils::setup_program_config_update_test().await;

    let rent_collector = Keypair::new();
    let mut instruction = finalize_config_update(
        &context.program_owner.pubkey(),
        &context.program_config_account.pubkey(),
        &context.multisig_op_account.pubkey(),
        &rent_collector.pubkey(),
        context.expected_config_update.clone(),
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
async fn test_wallet_creation() {
    let mut context = utils::setup_wallet_tests(None).await;

    utils::approve_or_deny_1_of_2_multisig_op(
        context.banks_client.borrow_mut(),
        &context.program_owner.pubkey(),
        &context.multisig_op_account.pubkey(),
        &context.approvers[0],
        &context.payer,
        &context.approvers[1].pubkey(),
        context.recent_blockhash,
        ApprovalDisposition::APPROVE
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
    let wallet_account = utils::finalize_wallet(context.borrow_mut()).await;

    // verify that it was created as expected
    let wallet_config = WalletConfig::unpack_from_slice(
        context
            .banks_client
            .get_account(wallet_account.pubkey())
            .await
            .unwrap()
            .unwrap()
            .data
            .as_slice(),
    )
    .unwrap();
    assert!(wallet_config.is_initialized);
    assert_eq!(
        wallet_config.wallet_guid_hash,
        context.wallet_guid_hash.as_slice()
    );
    assert_eq!(
        wallet_config.wallet_name_hash,
        context.wallet_name_hash.as_slice()
    );
    assert_eq!(
        wallet_config.approvers,
        vec![context.approvers[1].pubkey(), context.approvers[2].pubkey()]
    );
    assert_eq!(
        wallet_config.allowed_destinations,
        vec![context.allowed_destination]
    );
    assert_eq!(
        wallet_config.program_config_address,
        context.program_config_account.pubkey()
    );
    assert_eq!(wallet_config.approvals_required_for_transfer, 2);
    assert_eq!(wallet_config.approval_timeout_for_transfer, Duration::from_secs(1800));

    // verify the multisig op account is closed
    assert!(context
        .banks_client
        .get_account(context.multisig_op_account.pubkey())
        .await
        .unwrap()
        .is_none());
    // and that the remaining balance went to the rent collector (less the 10000 in signature fees for the finalize and the wallet_account_rent)
    let ending_rent_collector_balance = context
        .banks_client
        .get_balance(context.payer.pubkey())
        .await
        .unwrap();
    let rent = context.banks_client.get_rent().await.unwrap();
    let wallet_account_rent = rent.minimum_balance(WalletConfig::LEN);
    assert_eq!(
        starting_rent_collector_balance + op_account_balance - 10000 - wallet_account_rent,
        ending_rent_collector_balance
    );
}

#[tokio::test]
async fn test_wallet_creation_fails_if_time_out_not_set() {
    assert_eq!(
        utils::setup_init_wallet_failure_tests(None,
                                               2,
                                               Duration::from_secs(0),
                                               vec![Pubkey::new_unique()]).await,
        TransactionError::InstructionError(1, InvalidArgument)
    )
}

#[tokio::test]
async fn test_wallet_creation_fails_if_no_approvers() {
    assert_eq!(
        utils::setup_init_wallet_failure_tests(None,
                                               1,
                                               Duration::from_secs(18000),
                                               vec![]).await,
        TransactionError::InstructionError(1, InvalidArgument)
    )
}

#[tokio::test]
async fn test_wallet_creation_fails_if_num_approvals_required_not_set() {
    assert_eq!(
        utils::setup_init_wallet_failure_tests(None,
                                               0,
                                               Duration::from_secs(18000),
                                               vec![Pubkey::new_unique()]).await,
        TransactionError::InstructionError(1, InvalidArgument)
    )
}

#[tokio::test]
async fn test_wallet_creation_not_signed_by_rent_collector() {
    let mut context = utils::setup_wallet_tests(None).await;

    let rent_collector = Keypair::new();
    let wallet_account = Keypair::new();
    let rent = context.banks_client.get_rent().await.unwrap();
    let wallet_account_rent = rent.minimum_balance(WalletConfig::LEN);
    let mut instruction = finalize_wallet_creation(
        &context.program_owner.pubkey(),
        &context.program_config_account.pubkey(),
        &wallet_account.pubkey(),
        &context.multisig_op_account.pubkey(),
        &rent_collector.pubkey(),
        context.wallet_guid_hash,
        context.expected_config_update,
    );
    instruction.accounts[3].is_signer = false;

    let finalize_transaction = Transaction::new_signed_with_payer(
        &[
            system_instruction::create_account(
                &context.payer.pubkey(),
                &wallet_account.pubkey(),
                wallet_account_rent,
                WalletConfig::LEN as u64,
                &context.program_owner.pubkey(),
            ),
            instruction,
        ],
        Some(&context.payer.pubkey()),
        &[&context.payer, &wallet_account],
        context.recent_blockhash,
    );
    assert_eq!(
        context
            .banks_client
            .process_transaction(finalize_transaction)
            .await
            .unwrap_err()
            .unwrap(),
        TransactionError::InstructionError(1, MissingRequiredSignature),
    );
}

#[tokio::test]
async fn test_wallet_creation_incorrect_hash() {
    let mut context = utils::setup_wallet_tests(None).await;

    let wallet_account = Keypair::new();
    let rent = context.banks_client.get_rent().await.unwrap();
    let wallet_account_rent = rent.minimum_balance(WalletConfig::LEN);

    let mut wrong_guid_hash = context.wallet_guid_hash.clone();
    wrong_guid_hash.reverse();
    let finalize_transaction_wrong_wallet_guid_hash = Transaction::new_signed_with_payer(
        &[
            system_instruction::create_account(
                &context.payer.pubkey(),
                &wallet_account.pubkey(),
                wallet_account_rent,
                WalletConfig::LEN as u64,
                &context.program_owner.pubkey(),
            ),
            finalize_wallet_creation(
                &context.program_owner.pubkey(),
                &context.program_config_account.pubkey(),
                &wallet_account.pubkey(),
                &context.multisig_op_account.pubkey(),
                &context.payer.pubkey(),
                wrong_guid_hash,
                context.expected_config_update.clone(),
            ),
        ],
        Some(&context.payer.pubkey()),
        &[&context.payer, &wallet_account],
        context.recent_blockhash,
    );
    assert_eq!(
        context
            .banks_client
            .process_transaction(finalize_transaction_wrong_wallet_guid_hash)
            .await
            .unwrap_err()
            .unwrap(),
        TransactionError::InstructionError(1, Custom(WalletError::InvalidSignature as u32)),
    );
    let altered_config = context.expected_config_update.borrow_mut();
    altered_config.approvals_required_for_transfer = 0;
    let finalize_transaction_wrong_config_update = Transaction::new_signed_with_payer(
        &[
            system_instruction::create_account(
                &context.payer.pubkey(),
                &wallet_account.pubkey(),
                wallet_account_rent,
                WalletConfig::LEN as u64,
                &context.program_owner.pubkey(),
            ),
            finalize_wallet_creation(
                &context.program_owner.pubkey(),
                &context.program_config_account.pubkey(),
                &wallet_account.pubkey(),
                &context.multisig_op_account.pubkey(),
                &context.payer.pubkey(),
                context.wallet_guid_hash,
                altered_config.clone(),
            ),
        ],
        Some(&context.payer.pubkey()),
        &[&context.payer, &wallet_account],
        context.recent_blockhash,
    );
    assert_eq!(
        context
            .banks_client
            .process_transaction(finalize_transaction_wrong_config_update)
            .await
            .unwrap_err()
            .unwrap(),
        TransactionError::InstructionError(1, Custom(WalletError::InvalidSignature as u32)),
    );
}

#[tokio::test]
async fn test_wallet_config_update() {
    let mut context = utils::setup_wallet_tests(None).await;

    utils::approve_or_deny_1_of_2_multisig_op(
        context.banks_client.borrow_mut(),
        &context.program_owner.pubkey(),
        &context.multisig_op_account.pubkey(),
        &context.approvers[0],
        &context.payer,
        &context.approvers[1].pubkey(),
        context.recent_blockhash,
        ApprovalDisposition::APPROVE
    )
    .await;

    let wallet_account = utils::finalize_wallet(context.borrow_mut()).await;

    let rent = context.banks_client.get_rent().await.unwrap();
    let multisig_op_rent = rent.minimum_balance(MultisigOp::LEN);
    let multisig_op_account = Keypair::new();

    let wallet_name_hash = utils::hash_of(b"New Wallet Name");
    let destination = Pubkey::new_unique();
    let destination_name_hash = utils::hash_of(b"New Destination Name");
    let new_allowed_destination = AllowedDestination {
        address: destination,
        name_hash: destination_name_hash,
    };
    let wallet_config_transaction = Transaction::new_signed_with_payer(
        &[
            system_instruction::create_account(
                &context.payer.pubkey(),
                &multisig_op_account.pubkey(),
                multisig_op_rent,
                MultisigOp::LEN as u64,
                &context.program_owner.pubkey(),
            ),
            init_wallet_config_update(
                &context.program_owner.pubkey(),
                &context.program_config_account.pubkey(),
                &multisig_op_account.pubkey(),
                &context.assistant_account.pubkey(),
                &wallet_account.pubkey(),
                context.wallet_guid_hash,
                wallet_name_hash,
                1,
                Duration::from_secs(7200),
                vec![context.approvers[0].pubkey()],
                vec![context.approvers[1].pubkey()],
                vec![new_allowed_destination],
                vec![context.allowed_destination],
            ),
        ],
        Some(&context.payer.pubkey()),
        &[
            &context.payer,
            &multisig_op_account,
            &context.assistant_account,
        ],
        context.recent_blockhash,
    );
    context
        .banks_client
        .process_transaction(wallet_config_transaction)
        .await
        .unwrap();

    utils::approve_or_deny_1_of_2_multisig_op(
        context.banks_client.borrow_mut(),
        &context.program_owner.pubkey(),
        &multisig_op_account.pubkey(),
        &context.approvers[0],
        &context.payer,
        &context.approvers[1].pubkey(),
        context.recent_blockhash,
        ApprovalDisposition::APPROVE
    )
    .await;

    let expected_config_update = WalletConfigUpdate {
        name_hash: wallet_name_hash,
        approvals_required_for_transfer: 1,
        approval_timeout_for_transfer: Duration::from_secs(7200),
        add_approvers: vec![context.approvers[0].pubkey()],
        remove_approvers: vec![context.approvers[1].pubkey()],
        add_allowed_destinations: vec![new_allowed_destination],
        remove_allowed_destinations: vec![context.allowed_destination],
    };

    // finalize the config update
    let starting_rent_collector_balance = context
        .banks_client
        .get_balance(context.payer.pubkey())
        .await
        .unwrap();
    let op_account_balance = context
        .banks_client
        .get_balance(multisig_op_account.pubkey())
        .await
        .unwrap();
    let finalize_update = Transaction::new_signed_with_payer(
        &[finalize_wallet_config_update(
            &context.program_owner.pubkey(),
            &wallet_account.pubkey(),
            &multisig_op_account.pubkey(),
            &context.payer.pubkey(),
            context.wallet_guid_hash,
            expected_config_update,
        )],
        Some(&context.payer.pubkey()),
        &[&context.payer],
        context.recent_blockhash,
    );
    context
        .banks_client
        .process_transaction(finalize_update)
        .await
        .unwrap();

    // verify that it was updated as expected
    let wallet_config = WalletConfig::unpack_from_slice(
        context
            .banks_client
            .get_account(wallet_account.pubkey())
            .await
            .unwrap()
            .unwrap()
            .data
            .as_slice(),
    )
    .unwrap();
    assert!(wallet_config.is_initialized);
    assert_eq!(
        wallet_config.wallet_guid_hash,
        context.wallet_guid_hash.as_slice()
    );
    assert_eq!(wallet_config.approvals_required_for_transfer, 1);
    assert_eq!(wallet_config.approval_timeout_for_transfer, Duration::from_secs(7200));
    assert_eq!(wallet_config.wallet_name_hash, wallet_name_hash.as_slice());
    assert_eq!(
        wallet_config.approvers,
        vec![context.approvers[2].pubkey(), context.approvers[0].pubkey()]
    );
    assert_eq!(
        wallet_config.allowed_destinations,
        vec![new_allowed_destination]
    );
    assert_eq!(
        wallet_config.program_config_address,
        context.program_config_account.pubkey()
    );

    // verify the multisig op account is closed
    assert!(context
        .banks_client
        .get_account(multisig_op_account.pubkey())
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
async fn test_wallet_config_update_is_denied() {
    let mut context = utils::setup_wallet_tests(None).await;

    utils::approve_or_deny_1_of_2_multisig_op(
        context.banks_client.borrow_mut(),
        &context.program_owner.pubkey(),
        &context.multisig_op_account.pubkey(),
        &context.approvers[0],
        &context.payer,
        &context.approvers[1].pubkey(),
        context.recent_blockhash,
        ApprovalDisposition::APPROVE
    )
        .await;

    let wallet_account = utils::finalize_wallet(context.borrow_mut()).await;

    let rent = context.banks_client.get_rent().await.unwrap();
    let multisig_op_rent = rent.minimum_balance(MultisigOp::LEN);
    let multisig_op_account = Keypair::new();

    let wallet_name_hash = utils::hash_of(b"New Wallet Name");
    let destination = Pubkey::new_unique();
    let destination_name_hash = utils::hash_of(b"New Destination Name");
    let new_allowed_destination = AllowedDestination {
        address: destination,
        name_hash: destination_name_hash,
    };
    let wallet_config_transaction = Transaction::new_signed_with_payer(
        &[
            system_instruction::create_account(
                &context.payer.pubkey(),
                &multisig_op_account.pubkey(),
                multisig_op_rent,
                MultisigOp::LEN as u64,
                &context.program_owner.pubkey(),
            ),
            init_wallet_config_update(
                &context.program_owner.pubkey(),
                &context.program_config_account.pubkey(),
                &multisig_op_account.pubkey(),
                &context.assistant_account.pubkey(),
                &wallet_account.pubkey(),
                context.wallet_guid_hash,
                wallet_name_hash,
                1,
                Duration::from_secs(7200),
                vec![context.approvers[0].pubkey()],
                vec![context.approvers[1].pubkey()],
                vec![new_allowed_destination],
                vec![context.allowed_destination],
            ),
        ],
        Some(&context.payer.pubkey()),
        &[
            &context.payer,
            &multisig_op_account,
            &context.assistant_account,
        ],
        context.recent_blockhash,
    );
    context
        .banks_client
        .process_transaction(wallet_config_transaction)
        .await
        .unwrap();

    utils::approve_or_deny_1_of_2_multisig_op(
        context.banks_client.borrow_mut(),
        &context.program_owner.pubkey(),
        &multisig_op_account.pubkey(),
        &context.approvers[0],
        &context.payer,
        &context.approvers[1].pubkey(),
        context.recent_blockhash,
        ApprovalDisposition::DENY
    ).await;

    let expected_config_update = WalletConfigUpdate {
        name_hash: wallet_name_hash,
        approvals_required_for_transfer: 1,
        approval_timeout_for_transfer: Duration::from_secs(7200),
        add_approvers: vec![context.approvers[0].pubkey()],
        remove_approvers: vec![context.approvers[1].pubkey()],
        add_allowed_destinations: vec![new_allowed_destination],
        remove_allowed_destinations: vec![context.allowed_destination],
    };

    // finalize the config update
    let starting_rent_collector_balance = context
        .banks_client
        .get_balance(context.payer.pubkey())
        .await
        .unwrap();
    let op_account_balance = context
        .banks_client
        .get_balance(multisig_op_account.pubkey())
        .await
        .unwrap();
    let finalize_update = Transaction::new_signed_with_payer(
        &[finalize_wallet_config_update(
            &context.program_owner.pubkey(),
            &wallet_account.pubkey(),
            &multisig_op_account.pubkey(),
            &context.payer.pubkey(),
            context.wallet_guid_hash,
            expected_config_update,
        )],
        Some(&context.payer.pubkey()),
        &[&context.payer],
        context.recent_blockhash,
    );
    context
        .banks_client
        .process_transaction(finalize_update)
        .await
        .unwrap();

    // verify that it was updated as expected
    let wallet_config = WalletConfig::unpack_from_slice(
        context
            .banks_client
            .get_account(wallet_account.pubkey())
            .await
            .unwrap()
            .unwrap()
            .data
            .as_slice(),
    )
        .unwrap();
    assert!(wallet_config.is_initialized);
    assert_eq!(
        wallet_config.wallet_guid_hash,
        context.wallet_guid_hash.as_slice()
    );
    assert_eq!(wallet_config.approvals_required_for_transfer, 2);
    assert_eq!(wallet_config.approval_timeout_for_transfer, Duration::from_secs(1800));
    assert_eq!(wallet_config.wallet_name_hash, context.wallet_name_hash.as_slice());
    assert_eq!(
        wallet_config.approvers,
        vec![context.approvers[1].pubkey(), context.approvers[2].pubkey()]
    );
    assert_eq!(
        wallet_config.allowed_destinations,
        vec![context.allowed_destination]
    );
    assert_eq!(
        wallet_config.program_config_address,
        context.program_config_account.pubkey()
    );

    // verify the multisig op account is closed
    assert!(context
        .banks_client
        .get_account(multisig_op_account.pubkey())
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
async fn test_wallet_config_update_wrong_program_config_account() {
    let (mut context, wallet_account, _) = utils::setup_wallet_tests_and_finalize(None).await;

    let rent = context.banks_client.get_rent().await.unwrap();
    let program_rent = rent.minimum_balance(ProgramConfig::LEN);
    let multisig_account_rent = rent.minimum_balance(MultisigOp::LEN);
    let multisig_op_account = Keypair::new();

    let wrong_program_config_account = Keypair::new();
    context
        .banks_client
        .process_transaction(Transaction::new_signed_with_payer(
            &[system_instruction::create_account(
                &context.payer.pubkey(),
                &wrong_program_config_account.pubkey(),
                program_rent,
                ProgramConfig::LEN as u64,
                &context.program_owner.pubkey(),
            )],
            Some(&context.payer.pubkey()),
            &[&context.payer, &wrong_program_config_account],
            context.recent_blockhash,
        ))
        .await
        .unwrap();

    let wallet_config_transaction_bad_program_config_account = Transaction::new_signed_with_payer(
        &[
            system_instruction::create_account(
                &context.payer.pubkey(),
                &multisig_op_account.pubkey(),
                multisig_account_rent,
                MultisigOp::LEN as u64,
                &context.program_owner.pubkey(),
            ),
            init_wallet_config_update(
                &context.program_owner.pubkey(),
                &wrong_program_config_account.pubkey(),
                &multisig_op_account.pubkey(),
                &context.assistant_account.pubkey(),
                &wallet_account.pubkey(),
                context.wallet_guid_hash,
                context.wallet_name_hash,
                2,
                Duration::from_secs(7200),
                vec![],
                vec![],
                vec![],
                vec![],
            ),
        ],
        Some(&context.payer.pubkey()),
        &[
            &context.payer,
            &multisig_op_account,
            &context.assistant_account,
        ],
        context.recent_blockhash,
    );
    assert_eq!(
        context
            .banks_client
            .process_transaction(wallet_config_transaction_bad_program_config_account)
            .await
            .unwrap_err()
            .unwrap(),
        TransactionError::InstructionError(1, Custom(WalletError::InvalidConfigAccount as u32)),
    );
}

#[tokio::test]
async fn test_wallet_config_update_too_many_approvers() {
    let (mut context, wallet_account, _) = utils::setup_wallet_tests_and_finalize(None).await;

    let rent = context.banks_client.get_rent().await.unwrap();
    let multisig_account_rent = rent.minimum_balance(MultisigOp::LEN);
    let multisig_op_account = Keypair::new();

    let wallet_config_transaction_too_many_approvers = Transaction::new_signed_with_payer(
        &[
            system_instruction::create_account(
                &context.payer.pubkey(),
                &multisig_op_account.pubkey(),
                multisig_account_rent,
                MultisigOp::LEN as u64,
                &context.program_owner.pubkey(),
            ),
            init_wallet_config_update(
                &context.program_owner.pubkey(),
                &context.program_config_account.pubkey(),
                &multisig_op_account.pubkey(),
                &context.assistant_account.pubkey(),
                &wallet_account.pubkey(),
                context.wallet_guid_hash,
                context.wallet_name_hash,
                2,
                Duration::from_secs(7200),
                (1..ProgramConfig::MAX_APPROVERS)
                    .map(|_| Pubkey::new_unique())
                    .collect(),
                vec![],
                vec![],
                vec![],
            ),
        ],
        Some(&context.payer.pubkey()),
        &[
            &context.payer,
            &multisig_op_account,
            &context.assistant_account,
        ],
        context.recent_blockhash,
    );
    assert_eq!(
        context
            .banks_client
            .process_transaction(wallet_config_transaction_too_many_approvers)
            .await
            .unwrap_err()
            .unwrap(),
        TransactionError::InstructionError(1, InvalidArgument),
    );
}

#[tokio::test]
async fn test_wallet_config_update_too_many_destinations() {
    let (mut context, wallet_account, _) =
        utils::setup_wallet_tests_and_finalize(Some(100_000)).await;

    // we can't add all the destinations at once or we run out of memory
    let chunk = 10;
    let mut destination_count = 1;
    while destination_count < WalletConfig::MAX_DESTINATIONS {
        let (transaction, multisig_op_account, expected_config_update) =
            utils::add_n_destinations(context.borrow_mut(), &wallet_account.pubkey(), chunk).await;
        let result = context.banks_client.process_transaction(transaction).await;
        if destination_count > WalletConfig::MAX_DESTINATIONS {
            assert_eq!(
                result.unwrap_err().unwrap(),
                TransactionError::InstructionError(1, InvalidArgument)
            );
        } else {
            result.unwrap();

            utils::approve_or_deny_1_of_2_multisig_op(
                context.banks_client.borrow_mut(),
                &context.program_owner.pubkey(),
                &multisig_op_account.pubkey(),
                &context.approvers[0],
                &context.payer,
                &context.approvers[1].pubkey(),
                context.recent_blockhash,
                ApprovalDisposition::APPROVE
            )
            .await;

            let finalize_update = Transaction::new_signed_with_payer(
                &[finalize_wallet_config_update(
                    &context.program_owner.pubkey(),
                    &wallet_account.pubkey(),
                    &multisig_op_account.pubkey(),
                    &context.payer.pubkey(),
                    context.wallet_guid_hash,
                    expected_config_update,
                )],
                Some(&context.payer.pubkey()),
                &[&context.payer],
                context.recent_blockhash,
            );
            context
                .banks_client
                .process_transaction(finalize_update)
                .await
                .unwrap();
        }
        destination_count += chunk;
    }
}

#[tokio::test]
async fn test_wallet_config_update_too_many_required_approvers() {
    let (mut context, wallet_account, _) = utils::setup_wallet_tests_and_finalize(None).await;

    let rent = context.banks_client.get_rent().await.unwrap();
    let multisig_account_rent = rent.minimum_balance(MultisigOp::LEN);
    let multisig_op_account = Keypair::new();

    let wallet_config_transaction_too_many_required_approvers = Transaction::new_signed_with_payer(
        &[
            system_instruction::create_account(
                &context.payer.pubkey(),
                &multisig_op_account.pubkey(),
                multisig_account_rent,
                MultisigOp::LEN as u64,
                &context.program_owner.pubkey(),
            ),
            init_wallet_config_update(
                &context.program_owner.pubkey(),
                &context.program_config_account.pubkey(),
                &multisig_op_account.pubkey(),
                &context.assistant_account.pubkey(),
                &wallet_account.pubkey(),
                context.wallet_guid_hash,
                context.wallet_name_hash,
                3,
                Duration::from_secs(10800),
                vec![],
                vec![],
                vec![],
                vec![],
            ),
        ],
        Some(&context.payer.pubkey()),
        &[
            &context.payer,
            &multisig_op_account,
            &context.assistant_account,
        ],
        context.recent_blockhash,
    );
    assert_eq!(
        context
            .banks_client
            .process_transaction(wallet_config_transaction_too_many_required_approvers)
            .await
            .unwrap_err()
            .unwrap(),
        TransactionError::InstructionError(1, InvalidArgument),
    );
}

#[tokio::test]
async fn test_transfer_sol() {
    let (mut context, wallet_account, balance_account) =
        utils::setup_wallet_tests_and_finalize(None).await;
    let (multisig_op_account, result) = utils::setup_transfer_test(
        context.borrow_mut(),
        &wallet_account,
        &balance_account,
        None,
        None,
    )
    .await;
    result.unwrap();

    utils::approve_or_deny_n_of_n_multisig_op(
        context.banks_client.borrow_mut(),
        &context.program_owner.pubkey(),
        &multisig_op_account.pubkey(),
        vec![&context.approvers[1], &context.approvers[2]],
        &context.payer,
        context.recent_blockhash,
        ApprovalDisposition::APPROVE,
        OperationDisposition::APPROVED
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
                &context.program_owner.pubkey(),
                &multisig_op_account.pubkey(),
                &balance_account,
                &context.destination.pubkey(),
                &wallet_account.pubkey(),
                &context.payer.pubkey(),
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
    let (mut context, wallet_account, balance_account) =
        utils::setup_wallet_tests_and_finalize(None).await;
    let (multisig_op_account, result) = utils::setup_transfer_test(
        context.borrow_mut(),
        &wallet_account,
        &balance_account,
        None,
        None,
    )
        .await;
    result.unwrap();

    utils::approve_or_deny_n_of_n_multisig_op(
        context.banks_client.borrow_mut(),
        &context.program_owner.pubkey(),
        &multisig_op_account.pubkey(),
        vec![&context.approvers[1], &context.approvers[2]],
        &context.payer,
        context.recent_blockhash,
        ApprovalDisposition::DENY,
        OperationDisposition::DENIED
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
                &context.program_owner.pubkey(),
                &multisig_op_account.pubkey(),
                &balance_account,
                &context.destination.pubkey(),
                &wallet_account.pubkey(),
                &context.payer.pubkey(),
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
    let (mut context, wallet_account, balance_account) =
        utils::setup_wallet_tests_and_finalize(None).await;

    context.destination_name_hash = [0; 32];

    let (_, result) = utils::setup_transfer_test(
        context.borrow_mut(),
        &wallet_account,
        &balance_account,
        None,
        None,
    )
    .await;
    assert_eq!(
        result.unwrap_err().unwrap(),
        TransactionError::InstructionError(1, Custom(WalletError::DestinationNotAllowed as u32)),
    )
}

#[tokio::test]
async fn test_transfer_requires_multisig() {
    let (mut context, wallet_account, balance_account) =
        utils::setup_wallet_tests_and_finalize(None).await;
    let (multisig_op_account, result) = utils::setup_transfer_test(
        context.borrow_mut(),
        &wallet_account,
        &balance_account,
        None,
        None,
    )
    .await;
    result.unwrap();

    utils::approve_or_deny_1_of_2_multisig_op(
        context.banks_client.borrow_mut(),
        &context.program_owner.pubkey(),
        &multisig_op_account.pubkey(),
        &context.approvers[1],
        &context.payer,
        &context.approvers[2].pubkey(),
        context.recent_blockhash,
        ApprovalDisposition::APPROVE
    )
    .await;

    assert_eq!(
        context
            .banks_client
            .process_transaction(Transaction::new_signed_with_payer(
                &[finalize_transfer(
                    &context.program_owner.pubkey(),
                    &multisig_op_account.pubkey(),
                    &balance_account,
                    &context.destination.pubkey(),
                    &wallet_account.pubkey(),
                    &context.payer.pubkey(),
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
    let (mut context, wallet_account, balance_account) =
        utils::setup_wallet_tests_and_finalize(None).await;
    let (multisig_op_account, result) = utils::setup_transfer_test(
        context.borrow_mut(),
        &wallet_account,
        &balance_account,
        None,
        None,
    ).await;
    result.unwrap();


    assert_eq!(
        context
            .banks_client
            .process_transaction(Transaction::new_signed_with_payer(
                &[set_approval_disposition(
                    &context.program_owner.pubkey(),
                    &multisig_op_account.pubkey(),
                    &context.approvers[1].pubkey(),
                    ApprovalDisposition::APPROVE,
                    context.params_hash
                )],
                Some(&context.payer.pubkey()),
                &[&context.payer, &context.approvers[1]],
                context.recent_blockhash,
            ))
            .await
            .unwrap_err()
            .unwrap(),
        TransactionError::InstructionError(
            0,
            Custom(WalletError::InvalidSignature as u32)
        ),
    );

}

#[tokio::test]
async fn test_transfer_insufficient_balance() {
    let (mut context, wallet_account, balance_account) =
        utils::setup_wallet_tests_and_finalize(None).await;
    let (multisig_op_account, result) = utils::setup_transfer_test(
        context.borrow_mut(),
        &wallet_account,
        &balance_account,
        None,
        None,
    )
    .await;
    result.unwrap();

    utils::approve_or_deny_n_of_n_multisig_op(
        context.banks_client.borrow_mut(),
        &context.program_owner.pubkey(),
        &multisig_op_account.pubkey(),
        vec![&context.approvers[1], &context.approvers[2]],
        &context.payer,
        context.recent_blockhash,
        ApprovalDisposition::APPROVE,
        OperationDisposition::APPROVED
    )
    .await;

    assert_eq!(
        context
            .banks_client
            .process_transaction(Transaction::new_signed_with_payer(
                &[finalize_transfer(
                    &context.program_owner.pubkey(),
                    &multisig_op_account.pubkey(),
                    &balance_account,
                    &context.destination.pubkey(),
                    &wallet_account.pubkey(),
                    &context.payer.pubkey(),
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
    let (mut context, wallet_account, balance_account) =
        utils::setup_wallet_tests_and_finalize(None).await;

    // remove the whitelisted destination
    let rent = context.banks_client.get_rent().await.unwrap();
    let multisig_op_rent = rent.minimum_balance(MultisigOp::LEN);
    let multisig_op_account = Keypair::new();

    let wallet_config_transaction = Transaction::new_signed_with_payer(
        &[
            system_instruction::create_account(
                &context.payer.pubkey(),
                &multisig_op_account.pubkey(),
                multisig_op_rent,
                MultisigOp::LEN as u64,
                &context.program_owner.pubkey(),
            ),
            init_wallet_config_update(
                &context.program_owner.pubkey(),
                &context.program_config_account.pubkey(),
                &multisig_op_account.pubkey(),
                &context.assistant_account.pubkey(),
                &wallet_account.pubkey(),
                context.wallet_guid_hash,
                context.wallet_name_hash,
                2,
                Duration::from_secs(7200),
                vec![],
                vec![],
                vec![],
                vec![context.allowed_destination],
            ),
        ],
        Some(&context.payer.pubkey()),
        &[
            &context.payer,
            &multisig_op_account,
            &context.assistant_account,
        ],
        context.recent_blockhash,
    );
    context
        .banks_client
        .process_transaction(wallet_config_transaction)
        .await
        .unwrap();

    utils::approve_or_deny_1_of_2_multisig_op(
        context.banks_client.borrow_mut(),
        &context.program_owner.pubkey(),
        &multisig_op_account.pubkey(),
        &context.approvers[0],
        &context.payer,
        &context.approvers[1].pubkey(),
        context.recent_blockhash,
        ApprovalDisposition::APPROVE
    )
    .await;

    let expected_config_update = WalletConfigUpdate {
        name_hash: context.wallet_name_hash,
        approvals_required_for_transfer: 2,
        approval_timeout_for_transfer: Duration::from_secs(7200),
        add_approvers: vec![],
        remove_approvers: vec![],
        add_allowed_destinations: vec![],
        remove_allowed_destinations: vec![context.allowed_destination],
    };

    // finalize the config update
    let finalize_update = Transaction::new_signed_with_payer(
        &[finalize_wallet_config_update(
            &context.program_owner.pubkey(),
            &wallet_account.pubkey(),
            &multisig_op_account.pubkey(),
            &context.payer.pubkey(),
            context.wallet_guid_hash,
            expected_config_update,
        )],
        Some(&context.payer.pubkey()),
        &[&context.payer],
        context.recent_blockhash,
    );
    context
        .banks_client
        .process_transaction(finalize_update)
        .await
        .unwrap();

    let (_, result) = utils::setup_transfer_test(
        context.borrow_mut(),
        &wallet_account,
        &balance_account,
        None,
        None,
    )
    .await;
    assert_eq!(
        result.unwrap_err().unwrap(),
        TransactionError::InstructionError(1, Custom(WalletError::DestinationNotAllowed as u32)),
    );
}

#[tokio::test]
async fn test_transfer_spl() {
    let (mut context, wallet_account, balance_account) =
        utils::setup_wallet_tests_and_finalize(Some(30000)).await;

    let spl_context = utils::setup_spl_transfer_test(&mut context, &balance_account).await;

    let (multisig_op_account, result) = utils::setup_transfer_test(
        context.borrow_mut(),
        &wallet_account,
        &balance_account,
        Some(&spl_context.mint.pubkey()),
        None,
    )
    .await;
    result.unwrap();

    utils::approve_or_deny_n_of_n_multisig_op(
        context.banks_client.borrow_mut(),
        &context.program_owner.pubkey(),
        &multisig_op_account.pubkey(),
        vec![&context.approvers[1], &context.approvers[2]],
        &context.payer,
        context.recent_blockhash,
        ApprovalDisposition::APPROVE,
        OperationDisposition::APPROVED
    )
    .await;

    assert_eq!(
        utils::get_token_balance(&mut context, &spl_context.source_token_address).await,
        1000
    );
    assert_eq!(
        utils::get_token_balance(&mut context, &spl_context.destination_token_address).await,
        0
    );

    context
        .banks_client
        .process_transaction(Transaction::new_signed_with_payer(
            &[finalize_transfer(
                &context.program_owner.pubkey(),
                &multisig_op_account.pubkey(),
                &balance_account,
                &context.allowed_destination.address,
                &wallet_account.pubkey(),
                &context.payer.pubkey(),
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
        utils::get_token_balance(&mut context, &spl_context.source_token_address).await,
        1000 - 123
    );
    assert_eq!(
        utils::get_token_balance(&mut context, &spl_context.destination_token_address).await,
        123
    );
}

#[tokio::test]
async fn test_transfer_spl_insufficient_balance() {
    let (mut context, wallet_account, balance_account) =
        utils::setup_wallet_tests_and_finalize(Some(30000)).await;

    let spl_context = utils::setup_spl_transfer_test(&mut context, &balance_account).await;

    let (multisig_op_account, result) = utils::setup_transfer_test(
        context.borrow_mut(),
        &wallet_account,
        &balance_account,
        Some(&spl_context.mint.pubkey()),
        Some(1230),
    )
    .await;
    result.unwrap();

    utils::approve_or_deny_n_of_n_multisig_op(
        context.banks_client.borrow_mut(),
        &context.program_owner.pubkey(),
        &multisig_op_account.pubkey(),
        vec![&context.approvers[1], &context.approvers[2]],
        &context.payer,
        context.recent_blockhash,
        ApprovalDisposition::APPROVE,
        OperationDisposition::APPROVED
    )
    .await;

    assert_eq!(
        utils::get_token_balance(&mut context, &spl_context.source_token_address).await,
        1000
    );
    assert_eq!(
        utils::get_token_balance(&mut context, &spl_context.destination_token_address).await,
        0
    );

    assert_eq!(
        context
            .banks_client
            .process_transaction(Transaction::new_signed_with_payer(
                &[finalize_transfer(
                    &context.program_owner.pubkey(),
                    &multisig_op_account.pubkey(),
                    &balance_account,
                    &context.allowed_destination.address,
                    &wallet_account.pubkey(),
                    &context.payer.pubkey(),
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
        utils::get_token_balance(&mut context, &spl_context.source_token_address).await,
        1000
    );
    assert_eq!(
        utils::get_token_balance(&mut context, &spl_context.destination_token_address).await,
        0
    );
}
