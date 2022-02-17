#![cfg(feature = "test-bpf")]

mod common;

pub use common::instructions::*;
pub use common::utils::*;

use solana_program::instruction::InstructionError::{Custom, InvalidArgument};
use solana_program_test::tokio;
use solana_sdk::transaction::TransactionError;
use std::borrow::BorrowMut;
use strike_wallet::error::WalletError;
use strike_wallet::model::multisig_op::BooleanSetting;
use strike_wallet::utils::SlotId;

#[tokio::test]
async fn test_whitelist_status() {
    let (mut context, balance_account) = setup_balance_account_tests_and_finalize(None).await;

    // status is off by default
    verify_whitelist_status(&mut context, BooleanSetting::Off, 0).await;

    // transfer should go through
    let (_, result) = setup_transfer_test(context.borrow_mut(), &balance_account, None, None).await;
    result.unwrap();

    // add a whitelisted destination, should fail since whitelisting on
    let destination_to_add = context.allowed_destination;
    modify_whitelist(
        &mut context,
        vec![(SlotId::new(0), destination_to_add)],
        vec![],
        Some(Custom(WalletError::WhitelistDisabled as u32)),
    )
    .await;

    // turn whitelisting on should be able to add destination now
    account_settings_update(&mut context, Some(BooleanSetting::On), None, None).await;
    verify_whitelist_status(&mut context, BooleanSetting::On, 0).await;
    modify_whitelist(
        &mut context,
        vec![(SlotId::new(0), destination_to_add)],
        vec![],
        None,
    )
    .await;

    // try to turn it off - should fail since there are whitelisted destinations
    account_settings_update(
        &mut context,
        Some(BooleanSetting::Off),
        None,
        Some(InvalidArgument),
    )
    .await;

    // remove a whitelisted destination, status should still be On even though whitelist is empty
    let destination_to_remove = context.allowed_destination;
    modify_whitelist(
        &mut context,
        vec![],
        vec![(SlotId::new(0), destination_to_remove)],
        None,
    )
    .await;

    verify_whitelist_status(&mut context, BooleanSetting::On, 0).await;

    // make sure transfer fails
    let (_, result) = setup_transfer_test(context.borrow_mut(), &balance_account, None, None).await;
    assert_eq!(
        result.unwrap_err().unwrap(),
        TransactionError::InstructionError(1, Custom(WalletError::DestinationNotAllowed as u32)),
    );

    // explicitly turn it off and verify transfer succeeds
    account_settings_update(&mut context, Some(BooleanSetting::Off), None, None).await;
    verify_whitelist_status(&mut context, BooleanSetting::Off, 0).await;
    let (_, result) = setup_transfer_test(context.borrow_mut(), &balance_account, None, None).await;
    result.unwrap();

    // explicitly turn it on
    account_settings_update(&mut context, Some(BooleanSetting::On), None, None).await;
    verify_whitelist_status(&mut context, BooleanSetting::On, 0).await;
}
