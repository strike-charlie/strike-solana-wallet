#![cfg(feature = "test-bpf")]
use std::borrow::BorrowMut;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use solana_program::hash::Hash;
use solana_program::instruction::InstructionError::{
    Custom, InvalidArgument, MissingRequiredSignature,
};
use solana_program::system_program;
use solana_sdk::transaction::TransactionError;

use crate::utils::{
    assert_multisig_op_timestamps, get_program_config, hash_of, verify_multisig_op_init_fails,
    SignerKey, ToSet,
};
use itertools::Itertools;
use solana_program::instruction::InstructionError;
use std::collections::HashSet;
use strike_wallet::error::WalletError;
use strike_wallet::instruction::{
    finalize_config_update, finalize_transfer, finalize_wallet_config_update,
    finalize_wallet_creation, init_wallet_config_update, program_init_config_update,
    set_approval_disposition, WalletConfigUpdate,
};
use strike_wallet::model::multisig_op::{
    ApprovalDisposition, ApprovalDispositionRecord, OperationDisposition,
};
use strike_wallet::model::program_config::{AddressBook, Approvers, ProgramConfig, Signers};
use strike_wallet::model::signer::Signer;
use strike_wallet::model::wallet_config::AddressBookEntry;
use strike_wallet::utils::SlotId;
use {
    solana_program::system_instruction,
    solana_program_test::{processor, tokio, ProgramTest},
    solana_sdk::{
        account::ReadableAccount,
        program_pack::Pack,
        pubkey::Pubkey,
        signature::{Keypair, Signer as SdkSigner},
        transaction::Transaction,
    },
    strike_wallet::{
        model::multisig_op::{MultisigOp, MultisigOpParams},
        processor::Processor,
    },
};

mod utils;

#[tokio::test]
async fn init_program() {
    let approvals_required_for_config = 2;
    let approval_timeout_for_config = Duration::from_secs(3600);
    let signers = vec![
        (SlotId::new(0), Signer::new(Pubkey::new_unique())),
        (SlotId::new(1), Signer::new(Pubkey::new_unique())),
        (SlotId::new(2), Signer::new(Pubkey::new_unique())),
    ];
    let config_approvers = signers.clone();
    let address_book = vec![(
        SlotId::new(0),
        AddressBookEntry {
            address: Pubkey::new_unique(),
            name_hash: [0; 32],
        },
    )];

    let program_owner = Keypair::new();
    let mut pt = ProgramTest::new(
        "strike_wallet",
        program_owner.pubkey(),
        processor!(Processor::process),
    );
    pt.set_bpf_compute_max_units(25_000);
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
        Some(approvals_required_for_config),
        Some(signers.clone()),
        Some(config_approvers.clone()),
        Some(approval_timeout_for_config),
        Some(address_book.clone()),
    )
    .await
    .unwrap();

    let program_config =
        get_program_config(&mut banks_client, &program_config_account.pubkey()).await;
    assert_eq!(
        program_config,
        ProgramConfig {
            is_initialized: true,
            signers: Signers::from_vec(signers),
            assistant: assistant_account.pubkey_as_signer(),
            address_book: AddressBook::from_vec(address_book),
            approvals_required_for_config,
            approval_timeout_for_config,
            config_approvers: Approvers::from_enabled_vec(
                config_approvers
                    .into_iter()
                    .map(|(slot_id, _)| slot_id)
                    .collect_vec()
            ),
            wallets: Vec::new()
        }
    );
}

#[tokio::test]
async fn config_update() {
    let start = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
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
        multisig_op.disposition_records.to_set(),
        HashSet::from([
            ApprovalDispositionRecord {
                approver: context.approvers[0].pubkey(),
                disposition: ApprovalDisposition::NONE,
            },
            ApprovalDispositionRecord {
                approver: context.approvers[1].pubkey(),
                disposition: ApprovalDisposition::NONE,
            },
        ])
    );
    assert_eq!(multisig_op.dispositions_required, 1);
    assert_eq!(
        multisig_op.operation_disposition,
        OperationDisposition::NONE
    );
    assert_multisig_op_timestamps(&multisig_op, start, Duration::from_secs(3600));

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
        ApprovalDisposition::APPROVE,
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
    assert_eq!(
        multisig_op.operation_disposition,
        OperationDisposition::APPROVED
    );

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
    let program_config = get_program_config(
        &mut context.banks_client,
        &context.program_config_account.pubkey(),
    )
    .await;
    assert_eq!(context.expected_config_after_update, program_config);

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
async fn config_update_invalid_updates() {
    let program_owner = Keypair::new();
    let mut pt = ProgramTest::new(
        "strike_wallet",
        program_owner.pubkey(),
        processor!(Processor::process),
    );
    pt.set_bpf_compute_max_units(30_000);
    let (mut banks_client, payer, recent_blockhash) = pt.start().await;
    let program_config_account = Keypair::new();
    let assistant_account = Keypair::new();

    let approvers = vec![Keypair::new(), Keypair::new(), Keypair::new()];
    let signers = vec![
        approvers[0].pubkey_as_signer(),
        approvers[1].pubkey_as_signer(),
        approvers[2].pubkey_as_signer(),
    ];

    let address_book_entry = AddressBookEntry {
        address: Pubkey::new_unique(),
        name_hash: [0; 32],
    };
    let new_address_book_entry = AddressBookEntry {
        address: Pubkey::new_unique(),
        name_hash: [0; 32],
    };

    // first initialize the program config
    utils::init_program(
        &mut banks_client,
        &payer,
        recent_blockhash,
        &program_owner,
        &program_config_account,
        &assistant_account,
        Some(1),
        Some(vec![
            (SlotId::new(0), signers[0]),
            (SlotId::new(1), signers[1]),
        ]),
        Some(vec![
            (SlotId::new(0), signers[0]),
            (SlotId::new(1), signers[1]),
        ]),
        Some(Duration::from_secs(3600)),
        Some(vec![(SlotId::new(0), address_book_entry)]),
    )
    .await
    .unwrap();

    // verify approvals required for config can't exceed configured approvers count
    {
        let multisig_op_account = Keypair::new();
        verify_multisig_op_init_fails(
            &mut banks_client,
            recent_blockhash,
            &payer,
            &assistant_account,
            &multisig_op_account,
            program_init_config_update(
                &program_owner.pubkey(),
                &program_config_account.pubkey(),
                &multisig_op_account.pubkey(),
                &assistant_account.pubkey(),
                3,
                Duration::from_secs(7200),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ),
            InstructionError::InvalidArgument,
        )
        .await;
    }
    // verify it's not allowed to place a signer into a non-empty slot
    {
        let multisig_op_account = Keypair::new();
        verify_multisig_op_init_fails(
            &mut banks_client,
            recent_blockhash,
            &payer,
            &assistant_account,
            &multisig_op_account,
            program_init_config_update(
                &program_owner.pubkey(),
                &program_config_account.pubkey(),
                &multisig_op_account.pubkey(),
                &assistant_account.pubkey(),
                2,
                Duration::from_secs(7200),
                vec![(SlotId::new(0), signers[2])],
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ),
            InstructionError::InvalidArgument,
        )
        .await;
    }
    // verify it's not allowed to remove a signer from a slot when slot value does not match the provided one
    {
        let multisig_op_account = Keypair::new();
        verify_multisig_op_init_fails(
            &mut banks_client,
            recent_blockhash,
            &payer,
            &assistant_account,
            &multisig_op_account,
            program_init_config_update(
                &program_owner.pubkey(),
                &program_config_account.pubkey(),
                &multisig_op_account.pubkey(),
                &assistant_account.pubkey(),
                2,
                Duration::from_secs(7200),
                Vec::new(),
                vec![(SlotId::new(0), signers[2])],
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ),
            InstructionError::InvalidArgument,
        )
        .await;
    }
    // verify it's not allowed to place an address book entry into a non-empty slot
    {
        let multisig_op_account = Keypair::new();
        verify_multisig_op_init_fails(
            &mut banks_client,
            recent_blockhash,
            &payer,
            &assistant_account,
            &multisig_op_account,
            program_init_config_update(
                &program_owner.pubkey(),
                &program_config_account.pubkey(),
                &multisig_op_account.pubkey(),
                &assistant_account.pubkey(),
                2,
                Duration::from_secs(7200),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                vec![(SlotId::new(0), new_address_book_entry)],
                Vec::new(),
            ),
            InstructionError::InvalidArgument,
        )
        .await;
    }
    // verify it's not allowed to remove an address book entry from a slot when slot value does not match the provided one
    {
        let multisig_op_account = Keypair::new();
        verify_multisig_op_init_fails(
            &mut banks_client,
            recent_blockhash,
            &payer,
            &assistant_account,
            &multisig_op_account,
            program_init_config_update(
                &program_owner.pubkey(),
                &program_config_account.pubkey(),
                &multisig_op_account.pubkey(),
                &assistant_account.pubkey(),
                2,
                Duration::from_secs(7200),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                vec![(SlotId::new(0), new_address_book_entry)],
            ),
            InstructionError::InvalidArgument,
        )
        .await;
    }
    // verify it's not allowed to add a config approver that is not configured as signer
    {
        let multisig_op_account = Keypair::new();
        verify_multisig_op_init_fails(
            &mut banks_client,
            recent_blockhash,
            &payer,
            &assistant_account,
            &multisig_op_account,
            program_init_config_update(
                &program_owner.pubkey(),
                &program_config_account.pubkey(),
                &multisig_op_account.pubkey(),
                &assistant_account.pubkey(),
                2,
                Duration::from_secs(7200),
                Vec::new(),
                Vec::new(),
                vec![(SlotId::new(2), signers[2])],
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ),
            InstructionError::InvalidArgument,
        )
        .await;
    }
    // verify it's not allowed to add a config approver when provided slot value does not match the stored one
    {
        let multisig_op_account = Keypair::new();
        verify_multisig_op_init_fails(
            &mut banks_client,
            recent_blockhash,
            &payer,
            &assistant_account,
            &multisig_op_account,
            program_init_config_update(
                &program_owner.pubkey(),
                &program_config_account.pubkey(),
                &multisig_op_account.pubkey(),
                &assistant_account.pubkey(),
                2,
                Duration::from_secs(7200),
                Vec::new(),
                Vec::new(),
                vec![(SlotId::new(0), signers[2])],
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ),
            InstructionError::InvalidArgument,
        )
        .await;
    }
    // verify it's not allowed to remove a config approver when provided slot value does not match the stored one
    {
        let multisig_op_account = Keypair::new();
        verify_multisig_op_init_fails(
            &mut banks_client,
            recent_blockhash,
            &payer,
            &assistant_account,
            &multisig_op_account,
            program_init_config_update(
                &program_owner.pubkey(),
                &program_config_account.pubkey(),
                &multisig_op_account.pubkey(),
                &assistant_account.pubkey(),
                2,
                Duration::from_secs(7200),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                vec![(SlotId::new(0), signers[2])],
                Vec::new(),
                Vec::new(),
            ),
            InstructionError::InvalidArgument,
        )
        .await;
    }
}

#[tokio::test]
async fn config_update_is_denied() {
    let start = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let mut context = utils::setup_program_config_update_test().await;

    let initial_program_config = get_program_config(
        &mut context.banks_client,
        &context.program_config_account.pubkey(),
    )
    .await;

    utils::approve_or_deny_1_of_2_multisig_op(
        context.banks_client.borrow_mut(),
        &context.program_owner.pubkey(),
        &context.multisig_op_account.pubkey(),
        &context.approvers[0],
        &context.payer,
        &context.approvers[1].pubkey(),
        context.recent_blockhash,
        ApprovalDisposition::DENY,
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
    assert_eq!(
        multisig_op.operation_disposition,
        OperationDisposition::DENIED
    );
    assert_multisig_op_timestamps(&multisig_op, start, Duration::from_secs(3600));

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
    assert_eq!(
        initial_program_config,
        get_program_config(
            &mut context.banks_client,
            &context.program_config_account.pubkey()
        )
        .await
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
            context.params_hash,
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
    utils::finalize_wallet(context.borrow_mut()).await;

    // verify that it was created as expected
    let program_config = get_program_config(
        &mut context.banks_client,
        &context.program_config_account.pubkey(),
    )
    .await;
    let wallet_config = program_config
        .get_wallet_config(&context.wallet_guid_hash)
        .unwrap();

    assert_eq!(
        wallet_config.wallet_guid_hash,
        context.wallet_guid_hash.as_slice()
    );
    assert_eq!(
        wallet_config.wallet_name_hash,
        context.wallet_name_hash.as_slice()
    );
    assert_eq!(
        program_config
            .get_transfer_approvers_keys(wallet_config)
            .to_set(),
        HashSet::from([context.approvers[0].pubkey(), context.approvers[1].pubkey()])
    );
    assert_eq!(
        program_config
            .get_allowed_destinations(wallet_config)
            .to_set(),
        HashSet::from([context.allowed_destination])
    );
    assert_eq!(wallet_config.approvals_required_for_transfer, 2);
    assert_eq!(
        wallet_config.approval_timeout_for_transfer,
        Duration::from_secs(1800)
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
async fn test_wallet_creation_fails_if_time_out_not_set() {
    assert_eq!(
        utils::setup_init_wallet_failure_tests(
            None,
            2,
            Duration::from_secs(0),
            vec![Pubkey::new_unique()]
        )
        .await,
        TransactionError::InstructionError(1, InvalidArgument)
    )
}

#[tokio::test]
async fn test_wallet_creation_fails_if_no_approvers() {
    assert_eq!(
        utils::setup_init_wallet_failure_tests(None, 1, Duration::from_secs(18000), vec![]).await,
        TransactionError::InstructionError(1, InvalidArgument)
    )
}

#[tokio::test]
async fn test_wallet_creation_fails_if_num_approvals_required_not_set() {
    assert_eq!(
        utils::setup_init_wallet_failure_tests(
            None,
            0,
            Duration::from_secs(18000),
            vec![Pubkey::new_unique()]
        )
        .await,
        TransactionError::InstructionError(1, InvalidArgument)
    )
}

#[tokio::test]
async fn test_wallet_creation_not_signed_by_rent_collector() {
    let mut context = utils::setup_wallet_tests(None).await;

    let rent_collector = Keypair::new();
    let mut instruction = finalize_wallet_creation(
        &context.program_owner.pubkey(),
        &context.program_config_account.pubkey(),
        &context.multisig_op_account.pubkey(),
        &rent_collector.pubkey(),
        context.wallet_guid_hash,
        context.expected_config_update,
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
async fn test_wallet_creation_incorrect_hash() {
    let mut context = utils::setup_wallet_tests(None).await;

    let mut wrong_guid_hash = context.wallet_guid_hash.clone();
    wrong_guid_hash.reverse();
    let finalize_transaction_wrong_wallet_guid_hash = Transaction::new_signed_with_payer(
        &[finalize_wallet_creation(
            &context.program_owner.pubkey(),
            &context.program_config_account.pubkey(),
            &context.multisig_op_account.pubkey(),
            &context.payer.pubkey(),
            wrong_guid_hash,
            context.expected_config_update.clone(),
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
    let altered_config = context.expected_config_update.borrow_mut();
    altered_config.approvals_required_for_transfer = 0;
    let finalize_transaction_wrong_config_update = Transaction::new_signed_with_payer(
        &[finalize_wallet_creation(
            &context.program_owner.pubkey(),
            &context.program_config_account.pubkey(),
            &context.multisig_op_account.pubkey(),
            &context.payer.pubkey(),
            context.wallet_guid_hash,
            altered_config.clone(),
        )],
        Some(&context.payer.pubkey()),
        &[&context.payer],
        context.recent_blockhash,
    );
    assert_eq!(
        context
            .banks_client
            .process_transaction(finalize_transaction_wrong_config_update)
            .await
            .unwrap_err()
            .unwrap(),
        TransactionError::InstructionError(0, Custom(WalletError::InvalidSignature as u32)),
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
        ApprovalDisposition::APPROVE,
    )
    .await;

    utils::finalize_wallet(context.borrow_mut()).await;

    let program_config = get_program_config(
        &mut context.banks_client,
        &context.program_config_account.pubkey(),
    )
    .await;

    let rent = context.banks_client.get_rent().await.unwrap();
    let multisig_op_rent = rent.minimum_balance(MultisigOp::LEN);
    let multisig_op_account = Keypair::new();

    let wallet_name_hash = utils::hash_of(b"New Wallet Name");
    let destination_name_hash = utils::hash_of(b"Destination 2 Name");
    let new_allowed_destination = program_config
        .address_book
        .filled_slots()
        .into_iter()
        .find(|(_, addr_book_entry)| addr_book_entry.name_hash == destination_name_hash)
        .unwrap();

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
                context.wallet_guid_hash,
                wallet_name_hash,
                1,
                Duration::from_secs(7200),
                vec![(SlotId::new(2), context.approvers[2].pubkey_as_signer())],
                vec![(SlotId::new(0), context.approvers[0].pubkey_as_signer())],
                vec![new_allowed_destination],
                vec![(SlotId::new(0), context.allowed_destination)],
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
        ApprovalDisposition::APPROVE,
    )
    .await;

    let expected_config_update = WalletConfigUpdate {
        name_hash: wallet_name_hash,
        approvals_required_for_transfer: 1,
        approval_timeout_for_transfer: Duration::from_secs(7200),
        add_transfer_approvers: vec![(SlotId::new(2), context.approvers[2].pubkey_as_signer())],
        remove_transfer_approvers: vec![(SlotId::new(0), context.approvers[0].pubkey_as_signer())],
        add_allowed_destinations: vec![new_allowed_destination],
        remove_allowed_destinations: vec![(SlotId::new(0), context.allowed_destination)],
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
            &context.program_config_account.pubkey(),
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
    let config = get_program_config(
        &mut context.banks_client,
        &context.program_config_account.pubkey(),
    )
    .await;
    let wallet_config = config.get_wallet_config(&context.wallet_guid_hash).unwrap();

    assert_eq!(
        wallet_config.wallet_guid_hash,
        context.wallet_guid_hash.as_slice()
    );
    assert_eq!(wallet_config.approvals_required_for_transfer, 1);
    assert_eq!(
        wallet_config.approval_timeout_for_transfer,
        Duration::from_secs(7200)
    );
    assert_eq!(wallet_config.wallet_name_hash, wallet_name_hash.as_slice());
    assert_eq!(
        config.get_transfer_approvers_keys(wallet_config).to_set(),
        HashSet::from([context.approvers[1].pubkey(), context.approvers[2].pubkey()])
    );
    assert_eq!(
        config.get_allowed_destinations(wallet_config).to_set(),
        HashSet::from([new_allowed_destination.1])
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
        ApprovalDisposition::APPROVE,
    )
    .await;

    utils::finalize_wallet(context.borrow_mut()).await;

    let program_config = get_program_config(
        &mut context.banks_client,
        &context.program_config_account.pubkey(),
    )
    .await;
    let wallet_config = program_config
        .get_wallet_config(&context.wallet_guid_hash)
        .unwrap();

    let rent = context.banks_client.get_rent().await.unwrap();
    let multisig_op_rent = rent.minimum_balance(MultisigOp::LEN);
    let multisig_op_account = Keypair::new();

    let wallet_name_hash = utils::hash_of(b"New Wallet Name");
    let destination_name_hash = utils::hash_of(b"Destination 2 Name");
    let new_allowed_destination = program_config
        .address_book
        .filled_slots()
        .into_iter()
        .find(|(_, addr_book_entry)| addr_book_entry.name_hash == destination_name_hash)
        .unwrap();

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
                context.wallet_guid_hash,
                wallet_name_hash,
                1,
                Duration::from_secs(7200),
                vec![(SlotId::new(2), context.approvers[2].pubkey_as_signer())],
                vec![(SlotId::new(0), context.approvers[0].pubkey_as_signer())],
                vec![new_allowed_destination],
                vec![(SlotId::new(0), context.allowed_destination)],
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
        ApprovalDisposition::DENY,
    )
    .await;

    let expected_config_update = WalletConfigUpdate {
        name_hash: wallet_name_hash,
        approvals_required_for_transfer: 1,
        approval_timeout_for_transfer: Duration::from_secs(7200),
        add_transfer_approvers: vec![(SlotId::new(2), context.approvers[2].pubkey_as_signer())],
        remove_transfer_approvers: vec![(SlotId::new(0), context.approvers[0].pubkey_as_signer())],
        add_allowed_destinations: vec![new_allowed_destination],
        remove_allowed_destinations: vec![(SlotId::new(0), context.allowed_destination)],
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
            &context.program_config_account.pubkey(),
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

    // verify that wallet config was not changed
    let program_config_after_update = get_program_config(
        &mut context.banks_client,
        &context.program_config_account.pubkey(),
    )
    .await;
    let wallet_config_after_update = program_config
        .get_wallet_config(&context.wallet_guid_hash)
        .unwrap();

    assert_eq!(
        wallet_config_after_update.wallet_guid_hash,
        wallet_config.wallet_guid_hash
    );
    assert_eq!(
        wallet_config_after_update.approvals_required_for_transfer,
        wallet_config.approvals_required_for_transfer
    );
    assert_eq!(
        wallet_config_after_update.approval_timeout_for_transfer,
        wallet_config.approval_timeout_for_transfer
    );
    assert_eq!(
        wallet_config_after_update.wallet_name_hash,
        wallet_config.wallet_name_hash
    );
    assert_eq!(
        program_config_after_update
            .get_transfer_approvers_keys(wallet_config_after_update)
            .to_set(),
        program_config
            .get_transfer_approvers_keys(wallet_config)
            .to_set()
    );
    assert_eq!(
        program_config_after_update
            .get_allowed_destinations(wallet_config_after_update)
            .to_set(),
        program_config
            .get_allowed_destinations(wallet_config)
            .to_set()
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
async fn wallet_config_update_invalid_updates() {
    let (mut context, _) = utils::setup_wallet_tests_and_finalize(None).await;
    let program_config = get_program_config(
        &mut context.banks_client,
        &context.program_config_account.pubkey(),
    )
    .await;

    // verify error when updating non existing wallet
    {
        let wrong_wallet_guid_hash = [0; 32];
        let multisig_op_account = Keypair::new();
        verify_multisig_op_init_fails(
            &mut context.banks_client,
            context.recent_blockhash,
            &context.payer,
            &context.assistant_account,
            &multisig_op_account,
            init_wallet_config_update(
                &context.program_owner.pubkey(),
                &context.program_config_account.pubkey(),
                &multisig_op_account.pubkey(),
                &context.assistant_account.pubkey(),
                wrong_wallet_guid_hash,
                context.wallet_name_hash,
                2,
                Duration::from_secs(7200),
                vec![],
                vec![],
                vec![],
                vec![],
            ),
            Custom(WalletError::WalletNotFound as u32),
        )
        .await;
    }
    // verify approvals required for transfer can't exceed configured approvers count
    {
        let multisig_op_account = Keypair::new();
        verify_multisig_op_init_fails(
            &mut context.banks_client,
            context.recent_blockhash,
            &context.payer,
            &context.assistant_account,
            &multisig_op_account,
            init_wallet_config_update(
                &context.program_owner.pubkey(),
                &context.program_config_account.pubkey(),
                &multisig_op_account.pubkey(),
                &context.assistant_account.pubkey(),
                context.wallet_guid_hash,
                context.wallet_name_hash,
                3,
                Duration::from_secs(7200),
                vec![],
                vec![],
                vec![],
                vec![],
            ),
            InstructionError::InvalidArgument,
        )
        .await;
    }
    // verify not allowed to add transfer approver that is not configured as signer
    {
        let multisig_op_account = Keypair::new();
        verify_multisig_op_init_fails(
            &mut context.banks_client,
            context.recent_blockhash,
            &context.payer,
            &context.assistant_account,
            &multisig_op_account,
            init_wallet_config_update(
                &context.program_owner.pubkey(),
                &context.program_config_account.pubkey(),
                &multisig_op_account.pubkey(),
                &context.assistant_account.pubkey(),
                context.wallet_guid_hash,
                context.wallet_name_hash,
                2,
                Duration::from_secs(7200),
                vec![(SlotId::new(2), Keypair::new().pubkey_as_signer())],
                vec![],
                vec![],
                vec![],
            ),
            InstructionError::InvalidArgument,
        )
        .await;
    }
    // verify it's not allowed to add a transfer approver when provided slot value does not match the stored one
    {
        let multisig_op_account = Keypair::new();
        verify_multisig_op_init_fails(
            &mut context.banks_client,
            context.recent_blockhash,
            &context.payer,
            &context.assistant_account,
            &multisig_op_account,
            init_wallet_config_update(
                &context.program_owner.pubkey(),
                &context.program_config_account.pubkey(),
                &multisig_op_account.pubkey(),
                &context.assistant_account.pubkey(),
                context.wallet_guid_hash,
                context.wallet_name_hash,
                2,
                Duration::from_secs(7200),
                vec![(SlotId::new(0), context.approvers[1].pubkey_as_signer())],
                vec![],
                vec![],
                vec![],
            ),
            InstructionError::InvalidArgument,
        )
        .await;
    }
    // verify it's not allowed to remove a transfer approver when provided slot value does not match the stored one
    {
        let multisig_op_account = Keypair::new();
        verify_multisig_op_init_fails(
            &mut context.banks_client,
            context.recent_blockhash,
            &context.payer,
            &context.assistant_account,
            &multisig_op_account,
            init_wallet_config_update(
                &context.program_owner.pubkey(),
                &context.program_config_account.pubkey(),
                &multisig_op_account.pubkey(),
                &context.assistant_account.pubkey(),
                context.wallet_guid_hash,
                context.wallet_name_hash,
                2,
                Duration::from_secs(7200),
                vec![],
                vec![(SlotId::new(0), context.approvers[1].pubkey_as_signer())],
                vec![],
                vec![],
            ),
            InstructionError::InvalidArgument,
        )
        .await;
    }
    // verify not allowed to add an allowed destination that is not in the address book
    {
        let multisig_op_account = Keypair::new();
        verify_multisig_op_init_fails(
            &mut context.banks_client,
            context.recent_blockhash,
            &context.payer,
            &context.assistant_account,
            &multisig_op_account,
            init_wallet_config_update(
                &context.program_owner.pubkey(),
                &context.program_config_account.pubkey(),
                &multisig_op_account.pubkey(),
                &context.assistant_account.pubkey(),
                context.wallet_guid_hash,
                context.wallet_name_hash,
                2,
                Duration::from_secs(7200),
                vec![],
                vec![],
                vec![(
                    SlotId::new(2),
                    AddressBookEntry {
                        address: Keypair::new().pubkey(),
                        name_hash: hash_of(b"Destination 3"),
                    },
                )],
                vec![],
            ),
            InstructionError::InvalidArgument,
        )
        .await;
    }
    // verify it's not allowed to add an allowed destination when provided slot value does not match the stored one
    {
        let multisig_op_account = Keypair::new();
        verify_multisig_op_init_fails(
            &mut context.banks_client,
            context.recent_blockhash,
            &context.payer,
            &context.assistant_account,
            &multisig_op_account,
            init_wallet_config_update(
                &context.program_owner.pubkey(),
                &context.program_config_account.pubkey(),
                &multisig_op_account.pubkey(),
                &context.assistant_account.pubkey(),
                context.wallet_guid_hash,
                context.wallet_name_hash,
                2,
                Duration::from_secs(7200),
                vec![],
                vec![],
                vec![(
                    program_config.address_book.filled_slots()[0].0,
                    program_config.address_book.filled_slots()[1].1,
                )],
                vec![],
            ),
            InstructionError::InvalidArgument,
        )
        .await;
    }
    // verify it's not allowed to remove allowed destination when provided slot value does not match the stored one
    {
        let multisig_op_account = Keypair::new();
        verify_multisig_op_init_fails(
            &mut context.banks_client,
            context.recent_blockhash,
            &context.payer,
            &context.assistant_account,
            &multisig_op_account,
            init_wallet_config_update(
                &context.program_owner.pubkey(),
                &context.program_config_account.pubkey(),
                &multisig_op_account.pubkey(),
                &context.assistant_account.pubkey(),
                context.wallet_guid_hash,
                context.wallet_name_hash,
                2,
                Duration::from_secs(7200),
                vec![],
                vec![],
                vec![],
                vec![(
                    program_config.address_book.filled_slots()[0].0,
                    program_config.address_book.filled_slots()[1].1,
                )],
            ),
            InstructionError::InvalidArgument,
        )
        .await;
    }
}

#[tokio::test]
async fn test_transfer_sol() {
    let (mut context, balance_account) = utils::setup_wallet_tests_and_finalize(None).await;
    let (multisig_op_account, result) =
        utils::setup_transfer_test(context.borrow_mut(), &balance_account, None, None).await;
    result.unwrap();

    utils::approve_or_deny_n_of_n_multisig_op(
        context.banks_client.borrow_mut(),
        &context.program_owner.pubkey(),
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
                &context.program_owner.pubkey(),
                &multisig_op_account.pubkey(),
                &context.program_config_account.pubkey(),
                &balance_account,
                &context.destination.pubkey(),
                &context.payer.pubkey(),
                context.wallet_guid_hash,
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
    let (mut context, balance_account) = utils::setup_wallet_tests_and_finalize(None).await;
    let (multisig_op_account, result) =
        utils::setup_transfer_test(context.borrow_mut(), &balance_account, None, None).await;
    result.unwrap();

    utils::approve_or_deny_n_of_n_multisig_op(
        context.banks_client.borrow_mut(),
        &context.program_owner.pubkey(),
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
                &context.program_owner.pubkey(),
                &multisig_op_account.pubkey(),
                &context.program_config_account.pubkey(),
                &balance_account,
                &context.destination.pubkey(),
                &context.payer.pubkey(),
                context.wallet_guid_hash,
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
    let (mut context, balance_account) = utils::setup_wallet_tests_and_finalize(None).await;

    context.destination_name_hash = [0; 32];

    let (_, result) =
        utils::setup_transfer_test(context.borrow_mut(), &balance_account, None, None).await;
    assert_eq!(
        result.unwrap_err().unwrap(),
        TransactionError::InstructionError(1, Custom(WalletError::DestinationNotAllowed as u32)),
    )
}

#[tokio::test]
async fn test_transfer_requires_multisig() {
    let (mut context, balance_account) = utils::setup_wallet_tests_and_finalize(None).await;
    let (multisig_op_account, result) =
        utils::setup_transfer_test(context.borrow_mut(), &balance_account, None, None).await;
    result.unwrap();

    utils::approve_or_deny_1_of_2_multisig_op(
        context.banks_client.borrow_mut(),
        &context.program_owner.pubkey(),
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
                    &context.program_owner.pubkey(),
                    &multisig_op_account.pubkey(),
                    &context.program_config_account.pubkey(),
                    &balance_account,
                    &context.destination.pubkey(),
                    &context.payer.pubkey(),
                    context.wallet_guid_hash,
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
    let (mut context, balance_account) = utils::setup_wallet_tests_and_finalize(None).await;

    let (multisig_op_account, result) =
        utils::setup_transfer_test(context.borrow_mut(), &balance_account, None, None).await;
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
    let (mut context, balance_account) = utils::setup_wallet_tests_and_finalize(None).await;
    let (multisig_op_account, result) =
        utils::setup_transfer_test(context.borrow_mut(), &balance_account, None, None).await;
    result.unwrap();

    utils::approve_or_deny_n_of_n_multisig_op(
        context.banks_client.borrow_mut(),
        &context.program_owner.pubkey(),
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
                    &context.program_owner.pubkey(),
                    &multisig_op_account.pubkey(),
                    &context.program_config_account.pubkey(),
                    &balance_account,
                    &context.destination.pubkey(),
                    &context.payer.pubkey(),
                    context.wallet_guid_hash,
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
    let (mut context, balance_account) = utils::setup_wallet_tests_and_finalize(None).await;

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
                context.wallet_guid_hash,
                context.wallet_name_hash,
                2,
                Duration::from_secs(7200),
                vec![],
                vec![],
                vec![],
                vec![(SlotId::new(0), context.allowed_destination)],
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
        ApprovalDisposition::APPROVE,
    )
    .await;

    let expected_config_update = WalletConfigUpdate {
        name_hash: context.wallet_name_hash,
        approvals_required_for_transfer: 2,
        approval_timeout_for_transfer: Duration::from_secs(7200),
        add_transfer_approvers: vec![],
        remove_transfer_approvers: vec![],
        add_allowed_destinations: vec![],
        remove_allowed_destinations: vec![(SlotId::new(0), context.allowed_destination)],
    };

    // finalize the config update
    let finalize_update = Transaction::new_signed_with_payer(
        &[finalize_wallet_config_update(
            &context.program_owner.pubkey(),
            &context.program_config_account.pubkey(),
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

    let (_, result) =
        utils::setup_transfer_test(context.borrow_mut(), &balance_account, None, None).await;
    assert_eq!(
        result.unwrap_err().unwrap(),
        TransactionError::InstructionError(1, Custom(WalletError::DestinationNotAllowed as u32)),
    );
}

#[tokio::test]
async fn test_wrap_unwrap() {
    let (mut context, balance_account) = utils::setup_wallet_tests_and_finalize(Some(60000)).await;
    let rent = context.banks_client.get_rent().await.unwrap();
    let token_account_rent = rent.minimum_balance(spl_token::state::Account::LEN);
    let multisig_account_rent = rent.minimum_balance(MultisigOp::LEN);
    let wrapped_sol_account = spl_associated_token_account::get_associated_token_address(
        &balance_account,
        &spl_token::native_mint::id(),
    );

    let amount = 123;

    assert_eq!(
        utils::process_wrap(
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
        utils::process_wrap(
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

    utils::process_wrap(
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

    let result = utils::process_unwrapping(
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
    utils::process_unwrapping(
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
        utils::get_token_balance(&mut context, &wrapped_sol_account).await,
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
    let (mut context, balance_account) = utils::setup_wallet_tests_and_finalize(Some(50000)).await;

    let spl_context = utils::setup_spl_transfer_test(
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

    let (multisig_op_account, result) = utils::setup_transfer_test(
        context.borrow_mut(),
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
        vec![&context.approvers[0], &context.approvers[1]],
        &context.payer,
        context.recent_blockhash,
        ApprovalDisposition::APPROVE,
        OperationDisposition::APPROVED,
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
                &context.program_config_account.pubkey(),
                &balance_account,
                &context.allowed_destination.address,
                &context.payer.pubkey(),
                context.wallet_guid_hash,
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
    let (mut context, balance_account) = utils::setup_wallet_tests_and_finalize(Some(50_000)).await;
    let spl_context = utils::setup_spl_transfer_test(&mut context, &balance_account, true).await;

    let (multisig_op_account, result) = utils::setup_transfer_test(
        context.borrow_mut(),
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
        vec![&context.approvers[0], &context.approvers[1]],
        &context.payer,
        context.recent_blockhash,
        ApprovalDisposition::APPROVE,
        OperationDisposition::APPROVED,
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
                    &context.program_config_account.pubkey(),
                    &balance_account,
                    &context.allowed_destination.address,
                    &context.payer.pubkey(),
                    context.wallet_guid_hash,
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
