#![cfg(feature = "test-bpf")]

mod common;

pub use common::instructions::*;
pub use common::utils::*;

use std::borrow::BorrowMut;
use std::time::Duration;

use solana_program::instruction::InstructionError::Custom;

use crate::common::utils;
use common::instructions::{finalize_balance_account_update, init_balance_account_update};
use std::collections::HashSet;
use strike_wallet::error::WalletError;
use strike_wallet::instruction::BalanceAccountUpdate;
use strike_wallet::model::address_book::{AddressBookEntry, AddressBookEntryNameHash};
use strike_wallet::model::balance_account::{BalanceAccountGuidHash, BalanceAccountNameHash};
use strike_wallet::model::multisig_op::{ApprovalDisposition, BooleanSetting};
use strike_wallet::utils::SlotId;
use {
    solana_program::system_instruction,
    solana_program_test::tokio,
    solana_sdk::{
        program_pack::Pack,
        signature::{Keypair, Signer as SdkSigner},
        transaction::Transaction,
    },
    strike_wallet::model::multisig_op::MultisigOp,
};

#[tokio::test]
async fn test_balance_account_update() {
    let mut context = setup_balance_account_tests(Some(200000), false).await;

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

    account_settings_update(&mut context, Some(BooleanSetting::On), None, None).await;
    let destination_to_add = context.allowed_destination;
    modify_whitelist(
        &mut context,
        vec![(SlotId::new(0), destination_to_add)],
        vec![],
        None,
    )
    .await;

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

    account_settings_update(&mut context, Some(BooleanSetting::On), None, None).await;
    let destination_to_add = context.allowed_destination;
    modify_whitelist(
        &mut context,
        vec![(SlotId::new(0), destination_to_add)],
        vec![],
        None,
    )
    .await;

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
            Custom(WalletError::InvalidApproverCount as u32),
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
            Custom(WalletError::UnknownSigner as u32),
        )
        .await;
    }
    // verify it's not allowed to add a transfer approver that isn't configured as a signer.
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
            Custom(WalletError::UnknownSigner as u32),
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
            Custom(WalletError::InvalidSlot as u32),
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
            Custom(WalletError::InvalidSlot as u32),
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
            Custom(WalletError::InvalidSlot as u32),
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
            Custom(WalletError::InvalidSlot as u32),
        )
        .await;
    }
}

#[tokio::test]
async fn test_update_balance_account_name_happy_path() {
    let mut context = setup_balance_account_tests_and_finalize(None).await.0;
    let name_hash = BalanceAccountNameHash::new(&[1; 32]);

    update_balance_account_name_hash(&mut context, &name_hash).await;
    verify_balance_account_name_hash(&mut context, &name_hash).await;
}
