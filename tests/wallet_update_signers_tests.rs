#![cfg(feature = "test-bpf")]

mod common;

pub use common::instructions::*;
pub use common::utils::*;

use std::borrow::BorrowMut;

use solana_program::instruction::InstructionError::Custom;

use crate::common::utils;
use common::instructions::init_update_signer;
use strike_wallet::error::WalletError;
use strike_wallet::model::multisig_op::{ApprovalDisposition, SlotUpdateType};
use strike_wallet::model::wallet::Signers;
use strike_wallet::utils::SlotId;
use {
    solana_program_test::tokio,
    solana_sdk::signature::{Keypair, Signer as SdkSigner},
};

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
        Some(Custom(WalletError::SlotCannotBeInserted as u32)),
    )
    .await;

    // try to remove the signer from an occupied slot but give a wrong key
    update_signer(
        context.borrow_mut(),
        SlotUpdateType::Clear,
        1,
        signer_to_add_and_remove,
        None,
        Some(Custom(WalletError::SlotCannotBeRemoved as u32)),
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
        Some(Custom(WalletError::SignerIsConfigApprover as u32)),
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
        Custom(WalletError::SignerIsTransferApprover as u32),
    )
    .await;
}
