#![cfg(feature = "test-bpf")]

mod common;

pub use common::instructions::*;
pub use common::utils::*;

use std::borrow::BorrowMut;
use std::time::{Duration, SystemTime};

use solana_program::instruction::InstructionError::{Custom, MissingRequiredSignature};
use solana_sdk::transaction::TransactionError;

use crate::common::utils;
use common::instructions::{finalize_wallet_update, init_wallet_update, set_approval_disposition};
use itertools::Itertools;
use std::collections::HashSet;
use strike_wallet::error::WalletError;
use strike_wallet::model::address_book::{
    AddressBook, AddressBookEntry, AddressBookEntryNameHash, DAppBook,
};
use strike_wallet::model::multisig_op::{
    ApprovalDisposition, ApprovalDispositionRecord, OperationDisposition,
};
use strike_wallet::model::signer::Signer;
use strike_wallet::model::wallet::{Approvers, Signers, Wallet};
use strike_wallet::utils::SlotId;
use {
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
            config_policy_update_locked: false,
            dapp_book: DAppBook::from_vec(vec![]),
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
            Custom(WalletError::InvalidApproverCount as u32),
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
            Custom(WalletError::SlotCannotBeInserted as u32),
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
            Custom(WalletError::SlotCannotBeRemoved as u32),
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
            Custom(WalletError::SlotCannotBeInserted as u32),
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
            Custom(WalletError::SlotCannotBeRemoved as u32),
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
            Custom(WalletError::UnknownSigner as u32),
        )
        .await;
    }
    // verify it's not allowed to add a config approver who is not a signer.
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
            Custom(WalletError::UnknownSigner as u32),
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
            Custom(WalletError::InvalidSlot as u32),
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
