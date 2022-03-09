#![cfg(feature = "test-bpf")]

mod common;

pub use common::instructions::*;
pub use common::utils::*;

use solana_program::instruction::InstructionError::Custom;
use solana_program_test::tokio;
use solana_sdk::signer::Signer;
use strike_wallet::error::WalletError;
use strike_wallet::model::multisig_op::BooleanSetting;

#[tokio::test]
async fn test_address_book_update() {
    let (mut context, _) = setup_balance_account_tests_and_finalize(Some(32000)).await;

    let wallet = get_wallet(&mut context.banks_client, &context.wallet_account.pubkey()).await;

    let initial_entries = wallet.address_book.filled_slots().clone();
    verify_address_book(&mut context, initial_entries.clone(), vec![]).await;

    // remove all the entries
    modify_address_book_and_whitelist(
        &mut context,
        vec![],
        wallet.address_book.filled_slots(),
        vec![],
        vec![],
        None,
    )
    .await;
    verify_address_book(&mut context, vec![], vec![]).await;

    // turn white list on
    account_settings_update(&mut context, Some(BooleanSetting::On), None, None).await;

    // add 2 entries back and also whitelist
    modify_address_book_and_whitelist(
        &mut context,
        initial_entries.clone(),
        vec![],
        initial_entries.clone(),
        vec![],
        None,
    )
    .await;

    verify_address_book(
        &mut context,
        initial_entries.clone(),
        vec![
            initial_entries[0].1,
            initial_entries[1].1,
            initial_entries[2].1,
        ],
    )
    .await;

    // remove from whitelist and address book together
    modify_address_book_and_whitelist(
        &mut context,
        vec![],
        initial_entries.clone(),
        vec![],
        initial_entries.clone(),
        None,
    )
    .await;

    verify_address_book(&mut context, vec![], vec![]).await;

    // add 1 entry back and also whitelist
    modify_address_book_and_whitelist(
        &mut context,
        vec![initial_entries[0]],
        vec![],
        vec![initial_entries[0]],
        vec![],
        None,
    )
    .await;
    verify_address_book(
        &mut context,
        vec![initial_entries[0]],
        vec![initial_entries[0].1],
    )
    .await;

    // add and remove in same request
    modify_address_book_and_whitelist(
        &mut context,
        vec![initial_entries[1]],
        vec![initial_entries[0]],
        vec![initial_entries[1]],
        vec![initial_entries[0]],
        None,
    )
    .await;
    verify_address_book(
        &mut context,
        vec![initial_entries[1]],
        vec![initial_entries[1].1],
    )
    .await;
}

#[tokio::test]
async fn test_address_book_failures() {
    let (mut context, _) = setup_balance_account_tests_and_finalize(Some(32000)).await;

    let wallet = get_wallet(&mut context.banks_client, &context.wallet_account.pubkey()).await;

    // whitelist both entries, but whitelisting not on
    modify_address_book_and_whitelist(
        &mut context,
        vec![],
        vec![],
        wallet.address_book.filled_slots(),
        vec![],
        Some(Custom(WalletError::WhitelistDisabled as u32)),
    )
    .await;

    // turn on whitelisting and add the 2 entries
    account_settings_update(&mut context, Some(BooleanSetting::On), None, None).await;
    modify_address_book_and_whitelist(
        &mut context,
        vec![],
        vec![],
        wallet.address_book.filled_slots(),
        vec![],
        None,
    )
    .await;

    // try to remove a address book entry - will fail since its whitelisted
    modify_address_book_and_whitelist(
        &mut context,
        vec![],
        wallet.address_book.filled_slots(),
        vec![],
        vec![],
        Some(Custom(WalletError::DestinationInUse as u32)),
    )
    .await;

    // try to put a key into a filled slot with a different entry
    modify_address_book_and_whitelist(
        &mut context,
        vec![(
            wallet.address_book.filled_slots()[0].0,
            wallet.address_book.filled_slots()[1].1,
        )],
        vec![],
        vec![],
        vec![],
        Some(Custom(WalletError::SlotCannotBeInserted as u32)),
    )
    .await;

    // unwhitelist and remove first entry
    modify_address_book_and_whitelist(
        &mut context,
        vec![],
        vec![wallet.address_book.filled_slots()[0]],
        vec![],
        vec![wallet.address_book.filled_slots()[0]],
        None,
    )
    .await;

    // now try to whitelist - but its not in address book
    modify_address_book_and_whitelist(
        &mut context,
        vec![],
        vec![],
        vec![wallet.address_book.filled_slots()[0]],
        vec![],
        Some(Custom(WalletError::InvalidSlot as u32)),
    )
    .await;
}
