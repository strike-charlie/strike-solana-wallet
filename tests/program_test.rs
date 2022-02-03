#![cfg(feature = "test-bpf")]
mod common;
pub use common::utils::*;

use std::borrow::BorrowMut;
use std::time::{Duration, SystemTime};

use solana_program::hash::Hash;
use solana_program::instruction::InstructionError::{
    Custom, InvalidArgument, MissingRequiredSignature,
};
use solana_program::system_program;
use solana_sdk::transaction::TransactionError;

use crate::common::utils;
use itertools::Itertools;
use solana_program::instruction::InstructionError;
use std::collections::HashSet;
use strike_wallet::error::WalletError;
use strike_wallet::instruction::{
    finalize_balance_account_creation, finalize_balance_account_update, finalize_dapp_transaction,
    finalize_transfer, finalize_wallet_update, init_balance_account_update, init_dapp_transaction,
    init_transfer, init_update_signer, init_wallet_update, set_approval_disposition,
    BalanceAccountUpdate,
};
use strike_wallet::model::address_book::{AddressBook, AddressBookEntry, AddressBookEntryNameHash};
use strike_wallet::model::balance_account::{BalanceAccountGuidHash, BalanceAccountNameHash};
use strike_wallet::model::multisig_op::{
    ApprovalDisposition, ApprovalDispositionRecord, OperationDisposition, SlotUpdateType,
};
use strike_wallet::model::signer::Signer;
use strike_wallet::model::wallet::{Approvers, Signers, Wallet};
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

#[tokio::test]
async fn init_wallet() {
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
            name_hash: AddressBookEntryNameHash::zero(),
        },
    )];

    let program_id = Keypair::new().pubkey();
    let mut pt = ProgramTest::new("strike_wallet", program_id, processor!(Processor::process));
    pt.set_bpf_compute_max_units(25_000);
    let (mut banks_client, payer, recent_blockhash) = pt.start().await;
    let wallet_account = Keypair::new();
    let assistant_account = Keypair::new();

    utils::init_wallet(
        &mut banks_client,
        &payer,
        recent_blockhash,
        &program_id,
        &wallet_account,
        &assistant_account,
        Some(approvals_required_for_config),
        Some(signers.clone()),
        Some(config_approvers.clone()),
        Some(approval_timeout_for_config),
        Some(address_book.clone()),
    )
    .await
    .unwrap();

    assert_eq!(
        get_wallet(&mut banks_client, &wallet_account.pubkey()).await,
        Wallet {
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
            balance_accounts: Vec::new(),
            config_policy_update_locked: false
        }
    );
}

#[tokio::test]
async fn update_wallet() {
    let started_at = SystemTime::now();
    let mut context = setup_wallet_update_test().await;

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
    assert_multisig_op_timestamps(&multisig_op, started_at, Duration::from_secs(3600));

    assert_eq!(
        multisig_op.params_hash,
        MultisigOpParams::UpdateWallet {
            wallet_address: context.wallet_account.pubkey(),
            update: context.expected_update.clone(),
        }
        .hash()
    );

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
        &[finalize_wallet_update(
            &context.program_id,
            &context.wallet_account.pubkey(),
            &context.multisig_op_account.pubkey(),
            &context.payer.pubkey(),
            context.expected_update.clone(),
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

    // verify the wallet has been updated
    assert_eq!(
        context.expected_state_after_update,
        get_wallet(&mut context.banks_client, &context.wallet_account.pubkey()).await
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
async fn invalid_wallet_updates() {
    let program_id = Keypair::new().pubkey();
    let mut pt = ProgramTest::new("strike_wallet", program_id, processor!(Processor::process));
    pt.set_bpf_compute_max_units(30_000);
    let (mut banks_client, payer, recent_blockhash) = pt.start().await;
    let wallet_account = Keypair::new();
    let assistant_account = Keypair::new();

    let approvers = vec![Keypair::new(), Keypair::new(), Keypair::new()];
    let signers = vec![
        approvers[0].pubkey_as_signer(),
        approvers[1].pubkey_as_signer(),
        approvers[2].pubkey_as_signer(),
    ];

    let address_book_entry = AddressBookEntry {
        address: Pubkey::new_unique(),
        name_hash: AddressBookEntryNameHash::zero(),
    };
    let new_address_book_entry = AddressBookEntry {
        address: Pubkey::new_unique(),
        name_hash: AddressBookEntryNameHash::zero(),
    };

    // first initialize the wallet
    utils::init_wallet(
        &mut banks_client,
        &payer,
        recent_blockhash,
        &program_id,
        &wallet_account,
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
            init_wallet_update(
                &program_id,
                &wallet_account.pubkey(),
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
            init_wallet_update(
                &program_id,
                &wallet_account.pubkey(),
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
            init_wallet_update(
                &program_id,
                &wallet_account.pubkey(),
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
            init_wallet_update(
                &program_id,
                &wallet_account.pubkey(),
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
            init_wallet_update(
                &program_id,
                &wallet_account.pubkey(),
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
            init_wallet_update(
                &program_id,
                &wallet_account.pubkey(),
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
            init_wallet_update(
                &program_id,
                &wallet_account.pubkey(),
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
            init_wallet_update(
                &program_id,
                &wallet_account.pubkey(),
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
async fn wallet_update_is_denied() {
    let started_at = SystemTime::now();
    let mut context = setup_wallet_update_test().await;

    let initial_wallet_state =
        get_wallet(&mut context.banks_client, &context.wallet_account.pubkey()).await;

    approve_or_deny_1_of_2_multisig_op(
        context.banks_client.borrow_mut(),
        &context.program_id,
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
    assert_multisig_op_timestamps(&multisig_op, started_at, Duration::from_secs(3600));

    // finalize the multisig op
    let finalize_transaction = Transaction::new_signed_with_payer(
        &[finalize_wallet_update(
            &context.program_id,
            &context.wallet_account.pubkey(),
            &context.multisig_op_account.pubkey(),
            &context.payer.pubkey(),
            context.expected_update.clone(),
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

    // verify the wallet state has not been updated
    assert_eq!(
        initial_wallet_state,
        get_wallet(&mut context.banks_client, &context.wallet_account.pubkey()).await
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
async fn finalize_wallet_update_fails() {
    let mut context = setup_wallet_update_test().await;

    // attempt to finalize the multisig op
    let finalize_transaction = Transaction::new_signed_with_payer(
        &[finalize_wallet_update(
            &context.program_id,
            &context.wallet_account.pubkey(),
            &context.multisig_op_account.pubkey(),
            &context.payer.pubkey(),
            context.expected_update.clone(),
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
async fn wallet_update_invalid_approval() {
    let mut context = setup_wallet_update_test().await;

    // attempt to approve the update with an invalid approver
    let approve_transaction = Transaction::new_signed_with_payer(
        &[set_approval_disposition(
            &context.program_id,
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
async fn wallet_update_not_signed_by_rent_collector() {
    let mut context = setup_wallet_update_test().await;

    let rent_collector = Keypair::new();
    let mut instruction = finalize_wallet_update(
        &context.program_id,
        &context.wallet_account.pubkey(),
        &context.multisig_op_account.pubkey(),
        &rent_collector.pubkey(),
        context.expected_update.clone(),
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
async fn test_add_and_remove_signer() {
    let mut context = setup_wallet_update_test().await;

    let expected_signers_after_add = Signers::from_vec(vec![
        (SlotId::new(0), context.approvers[0].pubkey_as_signer()),
        (SlotId::new(1), context.approvers[1].pubkey_as_signer()),
        (SlotId::new(2), context.approvers[2].pubkey_as_signer()),
    ]);

    let signer_to_add_and_remove = context.approvers[2].pubkey_as_signer();

    update_signer(
        context.borrow_mut(),
        SlotUpdateType::SetIfEmpty,
        2,
        signer_to_add_and_remove,
        Some(expected_signers_after_add),
        None,
    )
    .await;

    let expected_signers_after_remove = Signers::from_vec(vec![
        (SlotId::new(0), context.approvers[0].pubkey_as_signer()),
        (SlotId::new(1), context.approvers[1].pubkey_as_signer()),
    ]);

    update_signer(
        context.borrow_mut(),
        SlotUpdateType::Clear,
        2,
        signer_to_add_and_remove,
        Some(expected_signers_after_remove),
        None,
    )
    .await;
}

#[tokio::test]
async fn test_add_and_remove_signer_init_failures() {
    let mut context = setup_wallet_update_test().await;

    let signer_to_add_and_remove = context.approvers[2].pubkey_as_signer();

    // put a signer in a slot already filled
    update_signer(
        context.borrow_mut(),
        SlotUpdateType::SetIfEmpty,
        1,
        signer_to_add_and_remove,
        None,
        Some(InvalidArgument),
    )
    .await;

    // try to remove the signer from an occupied slot but give a wrong key
    update_signer(
        context.borrow_mut(),
        SlotUpdateType::Clear,
        1,
        signer_to_add_and_remove,
        None,
        Some(InvalidArgument),
    )
    .await;

    // try to remove the signer that is a config approver
    let signer_to_add_and_remove = context.approvers[1].pubkey_as_signer();
    update_signer(
        context.borrow_mut(),
        SlotUpdateType::Clear,
        1,
        signer_to_add_and_remove,
        None,
        Some(InvalidArgument),
    )
    .await;
}

#[tokio::test]
async fn test_remove_signer_fails_for_a_transfer_approver() {
    let mut context = setup_balance_account_tests(None, true).await;

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
    utils::finalize_balance_account_creation(context.borrow_mut()).await;

    // approvers 0 & 1 are config and transfer approvers, 2 is just a transfer approver
    let multisig_op_account = Keypair::new();
    verify_multisig_op_init_fails(
        &mut context.banks_client,
        context.recent_blockhash,
        &context.payer,
        &context.assistant_account,
        &multisig_op_account,
        init_update_signer(
            &context.program_id,
            &context.wallet_account.pubkey(),
            &multisig_op_account.pubkey(),
            &context.assistant_account.pubkey(),
            SlotUpdateType::Clear,
            SlotId::new(2),
            context.approvers[2].pubkey_as_signer(),
        ),
        InstructionError::InvalidArgument,
    )
    .await;
}

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
        HashSet::from([context.allowed_destination])
    );
    assert_eq!(balance_account.approvals_required_for_transfer, 2);
    assert_eq!(
        balance_account.approval_timeout_for_transfer,
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
        TransactionError::InstructionError(1, InvalidArgument)
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
        TransactionError::InstructionError(1, InvalidArgument)
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

#[tokio::test]
async fn test_balance_account_update() {
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

    utils::finalize_balance_account_creation(context.borrow_mut()).await;

    let wallet = get_wallet(&mut context.banks_client, &context.wallet_account.pubkey()).await;

    let rent = context.banks_client.get_rent().await.unwrap();
    let multisig_op_rent = rent.minimum_balance(MultisigOp::LEN);
    let multisig_op_account = Keypair::new();

    let balance_account_name_hash = BalanceAccountNameHash::new(&hash_of(b"New Wallet Name"));
    let destination_name_hash = AddressBookEntryNameHash::new(&hash_of(b"Destination 2 Name"));
    let new_allowed_destination = wallet
        .address_book
        .filled_slots()
        .into_iter()
        .find(|(_, addr_book_entry)| addr_book_entry.name_hash == destination_name_hash)
        .unwrap();

    let balance_account_update_transaction = Transaction::new_signed_with_payer(
        &[
            system_instruction::create_account(
                &context.payer.pubkey(),
                &multisig_op_account.pubkey(),
                multisig_op_rent,
                MultisigOp::LEN as u64,
                &context.program_id,
            ),
            init_balance_account_update(
                &context.program_id,
                &context.wallet_account.pubkey(),
                &multisig_op_account.pubkey(),
                &context.assistant_account.pubkey(),
                context.balance_account_guid_hash,
                balance_account_name_hash,
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
        .process_transaction(balance_account_update_transaction)
        .await
        .unwrap();

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

    let expected_update = BalanceAccountUpdate {
        name_hash: balance_account_name_hash,
        approvals_required_for_transfer: 1,
        approval_timeout_for_transfer: Duration::from_secs(7200),
        add_transfer_approvers: vec![(SlotId::new(2), context.approvers[2].pubkey_as_signer())],
        remove_transfer_approvers: vec![(SlotId::new(0), context.approvers[0].pubkey_as_signer())],
        add_allowed_destinations: vec![new_allowed_destination],
        remove_allowed_destinations: vec![(SlotId::new(0), context.allowed_destination)],
    };

    // finalize the update
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
        &[finalize_balance_account_update(
            &context.program_id,
            &context.wallet_account.pubkey(),
            &multisig_op_account.pubkey(),
            &context.payer.pubkey(),
            context.balance_account_guid_hash,
            expected_update,
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
    let updated_wallet =
        get_wallet(&mut context.banks_client, &context.wallet_account.pubkey()).await;
    let updated_balance_account = updated_wallet
        .get_balance_account(&context.balance_account_guid_hash)
        .unwrap();

    assert_eq!(
        updated_balance_account.guid_hash,
        context.balance_account_guid_hash
    );
    assert_eq!(updated_balance_account.approvals_required_for_transfer, 1);
    assert_eq!(
        updated_balance_account.approval_timeout_for_transfer,
        Duration::from_secs(7200)
    );
    assert_eq!(updated_balance_account.name_hash, balance_account_name_hash);
    assert_eq!(
        updated_wallet
            .get_transfer_approvers_keys(updated_balance_account)
            .to_set(),
        HashSet::from([context.approvers[1].pubkey(), context.approvers[2].pubkey()])
    );
    assert_eq!(
        updated_wallet
            .get_allowed_destinations(updated_balance_account)
            .to_set(),
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
async fn test_balance_account_update_is_denied() {
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

    utils::finalize_balance_account_creation(context.borrow_mut()).await;

    let wallet = get_wallet(&mut context.banks_client, &context.wallet_account.pubkey()).await;
    let balance_account = wallet
        .get_balance_account(&context.balance_account_guid_hash)
        .unwrap();

    let rent = context.banks_client.get_rent().await.unwrap();
    let multisig_op_rent = rent.minimum_balance(MultisigOp::LEN);
    let multisig_op_account = Keypair::new();

    let balance_account_name_hash = BalanceAccountNameHash::new(&hash_of(b"New Wallet Name"));
    let destination_name_hash = AddressBookEntryNameHash::new(&hash_of(b"Destination 2 Name"));
    let new_allowed_destination = wallet
        .address_book
        .filled_slots()
        .into_iter()
        .find(|(_, addr_book_entry)| addr_book_entry.name_hash == destination_name_hash)
        .unwrap();

    let balance_account_update_transaction = Transaction::new_signed_with_payer(
        &[
            system_instruction::create_account(
                &context.payer.pubkey(),
                &multisig_op_account.pubkey(),
                multisig_op_rent,
                MultisigOp::LEN as u64,
                &context.program_id,
            ),
            init_balance_account_update(
                &context.program_id,
                &context.wallet_account.pubkey(),
                &multisig_op_account.pubkey(),
                &context.assistant_account.pubkey(),
                context.balance_account_guid_hash,
                balance_account_name_hash,
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
        .process_transaction(balance_account_update_transaction)
        .await
        .unwrap();

    approve_or_deny_1_of_2_multisig_op(
        context.banks_client.borrow_mut(),
        &context.program_id,
        &multisig_op_account.pubkey(),
        &context.approvers[0],
        &context.payer,
        &context.approvers[1].pubkey(),
        context.recent_blockhash,
        ApprovalDisposition::DENY,
    )
    .await;

    let expected_update = BalanceAccountUpdate {
        name_hash: balance_account_name_hash,
        approvals_required_for_transfer: 1,
        approval_timeout_for_transfer: Duration::from_secs(7200),
        add_transfer_approvers: vec![(SlotId::new(2), context.approvers[2].pubkey_as_signer())],
        remove_transfer_approvers: vec![(SlotId::new(0), context.approvers[0].pubkey_as_signer())],
        add_allowed_destinations: vec![new_allowed_destination],
        remove_allowed_destinations: vec![(SlotId::new(0), context.allowed_destination)],
    };

    // finalize the update
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
        &[finalize_balance_account_update(
            &context.program_id,
            &context.wallet_account.pubkey(),
            &multisig_op_account.pubkey(),
            &context.payer.pubkey(),
            context.balance_account_guid_hash,
            expected_update,
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

    // verify that balance account was not changed
    let wallet_after_update =
        get_wallet(&mut context.banks_client, &context.wallet_account.pubkey()).await;
    let balance_account_after_update = wallet
        .get_balance_account(&context.balance_account_guid_hash)
        .unwrap();

    assert_eq!(
        balance_account_after_update.guid_hash,
        balance_account.guid_hash
    );
    assert_eq!(
        balance_account_after_update.approvals_required_for_transfer,
        balance_account.approvals_required_for_transfer
    );
    assert_eq!(
        balance_account_after_update.approval_timeout_for_transfer,
        balance_account.approval_timeout_for_transfer
    );
    assert_eq!(
        balance_account_after_update.name_hash,
        balance_account.name_hash
    );
    assert_eq!(
        wallet_after_update
            .get_transfer_approvers_keys(balance_account_after_update)
            .to_set(),
        wallet.get_transfer_approvers_keys(balance_account).to_set()
    );
    assert_eq!(
        wallet_after_update
            .get_allowed_destinations(balance_account_after_update)
            .to_set(),
        wallet.get_allowed_destinations(balance_account).to_set()
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
async fn invalid_balance_account_updates() {
    let (mut context, _) = setup_balance_account_tests_and_finalize(None).await;
    let wallet = get_wallet(&mut context.banks_client, &context.wallet_account.pubkey()).await;

    // verify error when updating non existing balance account
    {
        let wrong_balance_account_guid_hash = BalanceAccountGuidHash::zero();
        let multisig_op_account = Keypair::new();
        verify_multisig_op_init_fails(
            &mut context.banks_client,
            context.recent_blockhash,
            &context.payer,
            &context.assistant_account,
            &multisig_op_account,
            init_balance_account_update(
                &context.program_id,
                &context.wallet_account.pubkey(),
                &multisig_op_account.pubkey(),
                &context.assistant_account.pubkey(),
                wrong_balance_account_guid_hash,
                context.balance_account_name_hash,
                2,
                Duration::from_secs(7200),
                vec![],
                vec![],
                vec![],
                vec![],
            ),
            Custom(WalletError::BalanceAccountNotFound as u32),
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
            init_balance_account_update(
                &context.program_id,
                &context.wallet_account.pubkey(),
                &multisig_op_account.pubkey(),
                &context.assistant_account.pubkey(),
                context.balance_account_guid_hash,
                context.balance_account_name_hash,
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
            init_balance_account_update(
                &context.program_id,
                &context.wallet_account.pubkey(),
                &multisig_op_account.pubkey(),
                &context.assistant_account.pubkey(),
                context.balance_account_guid_hash,
                context.balance_account_name_hash,
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
            init_balance_account_update(
                &context.program_id,
                &context.wallet_account.pubkey(),
                &multisig_op_account.pubkey(),
                &context.assistant_account.pubkey(),
                context.balance_account_guid_hash,
                context.balance_account_name_hash,
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
            init_balance_account_update(
                &context.program_id,
                &context.wallet_account.pubkey(),
                &multisig_op_account.pubkey(),
                &context.assistant_account.pubkey(),
                context.balance_account_guid_hash,
                context.balance_account_name_hash,
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
            init_balance_account_update(
                &context.program_id,
                &context.wallet_account.pubkey(),
                &multisig_op_account.pubkey(),
                &context.assistant_account.pubkey(),
                context.balance_account_guid_hash,
                context.balance_account_name_hash,
                2,
                Duration::from_secs(7200),
                vec![],
                vec![],
                vec![(
                    SlotId::new(2),
                    AddressBookEntry {
                        address: Keypair::new().pubkey(),
                        name_hash: AddressBookEntryNameHash::new(&hash_of(b"Destination 3")),
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
            init_balance_account_update(
                &context.program_id,
                &context.wallet_account.pubkey(),
                &multisig_op_account.pubkey(),
                &context.assistant_account.pubkey(),
                context.balance_account_guid_hash,
                context.balance_account_name_hash,
                2,
                Duration::from_secs(7200),
                vec![],
                vec![],
                vec![(
                    wallet.address_book.filled_slots()[0].0,
                    wallet.address_book.filled_slots()[1].1,
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
            init_balance_account_update(
                &context.program_id,
                &context.wallet_account.pubkey(),
                &multisig_op_account.pubkey(),
                &context.assistant_account.pubkey(),
                context.balance_account_guid_hash,
                context.balance_account_name_hash,
                2,
                Duration::from_secs(7200),
                vec![],
                vec![],
                vec![],
                vec![(
                    wallet.address_book.filled_slots()[0].0,
                    wallet.address_book.filled_slots()[1].1,
                )],
            ),
            InstructionError::InvalidArgument,
        )
        .await;
    }
}

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

    // remove the whitelisted destination
    let rent = context.banks_client.get_rent().await.unwrap();
    let multisig_op_rent = rent.minimum_balance(MultisigOp::LEN);
    let multisig_op_account = Keypair::new();

    let balance_account_update_transaction = Transaction::new_signed_with_payer(
        &[
            system_instruction::create_account(
                &context.payer.pubkey(),
                &multisig_op_account.pubkey(),
                multisig_op_rent,
                MultisigOp::LEN as u64,
                &context.program_id,
            ),
            init_balance_account_update(
                &context.program_id,
                &context.wallet_account.pubkey(),
                &multisig_op_account.pubkey(),
                &context.assistant_account.pubkey(),
                context.balance_account_guid_hash,
                context.balance_account_name_hash,
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
        .process_transaction(balance_account_update_transaction)
        .await
        .unwrap();

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

    let expected_config_update = BalanceAccountUpdate {
        name_hash: context.balance_account_name_hash,
        approvals_required_for_transfer: 2,
        approval_timeout_for_transfer: Duration::from_secs(7200),
        add_transfer_approvers: vec![],
        remove_transfer_approvers: vec![],
        add_allowed_destinations: vec![],
        remove_allowed_destinations: vec![(SlotId::new(0), context.allowed_destination)],
    };

    // finalize the config update
    let finalize_update = Transaction::new_signed_with_payer(
        &[finalize_balance_account_update(
            &context.program_id,
            &context.wallet_account.pubkey(),
            &multisig_op_account.pubkey(),
            &context.payer.pubkey(),
            context.balance_account_guid_hash,
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

    let (_, result) = setup_transfer_test(context.borrow_mut(), &balance_account, None, None).await;
    assert_eq!(
        result.unwrap_err().unwrap(),
        TransactionError::InstructionError(1, Custom(WalletError::DestinationNotAllowed as u32)),
    );
}

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
            &context.program_owner.pubkey(),
        ),
        init_transfer(
            &context.program_owner.pubkey(),
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
                    &context.program_owner.pubkey(),
                ),
                init_dapp_transaction(
                    &context.program_owner.pubkey(),
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
                    &context.program_owner.pubkey(),
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

    let params_hash = get_operation_hash(
        context.banks_client.borrow_mut(),
        multisig_op_account.pubkey(),
    )
    .await;
    let approver = &context.approvers[0];
    let approve_transaction = Transaction::new_signed_with_payer(
        &[set_approval_disposition(
            &context.program_owner.pubkey(),
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
                &context.program_owner.pubkey(),
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
                    &context.program_owner.pubkey(),
                ),
                init_dapp_transaction(
                    &context.program_owner.pubkey(),
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
                    &context.program_owner.pubkey(),
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
