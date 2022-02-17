#![cfg(feature = "test-bpf")]
mod common;
pub use common::instructions::*;
pub use common::utils::*;

pub use common::utils;
use solana_program_test::tokio;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer as SdkSigner;
use std::time::{Duration, SystemTime};
use strike_wallet::instruction::DAppBookUpdate;
use strike_wallet::model::address_book::{AddressBookEntry, AddressBookEntryNameHash};
use strike_wallet::model::multisig_op::{
    ApprovalDisposition, ApprovalDispositionRecord, MultisigOpParams, OperationDisposition,
};
use strike_wallet::utils::{SlotId, Slots};

#[tokio::test]
async fn test_dapp_book_update() {
    let started_at = SystemTime::now();
    let mut context = setup_test(30_000).await;

    let wallet_account = Keypair::new();
    let assistant_account = Keypair::new();

    let approvers = vec![Keypair::new()];
    let signers = vec![approvers[0].pubkey_as_signer()];

    utils::init_wallet(
        &mut context.banks_client,
        &context.payer,
        context.recent_blockhash,
        &context.program_id,
        &wallet_account,
        &assistant_account,
        Some(1),
        Some(vec![(SlotId::new(0), signers[0])]),
        Some(vec![(SlotId::new(0), signers[0])]),
        Some(Duration::from_secs(3600)),
        None,
    )
    .await
    .unwrap();

    // add a dapp to dapp book
    let dapp_program_id = Keypair::new().pubkey();
    let dapp_slot = (
        SlotId::new(0),
        AddressBookEntry {
            address: dapp_program_id,
            name_hash: AddressBookEntryNameHash::new(&hash_of(b"DApp Name")),
        },
    );

    let add_dapp = DAppBookUpdate {
        add_dapps: vec![dapp_slot],
        remove_dapps: vec![],
    };

    let multisig_op_account = utils::init_dapp_book_update(
        &mut context,
        wallet_account.pubkey(),
        &assistant_account,
        add_dapp.clone(),
    )
    .await
    .unwrap();

    assert_initialized_multisig_op(
        &get_multisig_op_data(&mut context.banks_client, multisig_op_account).await,
        started_at,
        Duration::from_secs(3600),
        1,
        &vec![ApprovalDispositionRecord {
            approver: approvers[0].pubkey(),
            disposition: ApprovalDisposition::NONE,
        }],
        OperationDisposition::NONE,
        &MultisigOpParams::UpdateDAppBook {
            wallet_address: wallet_account.pubkey(),
            update: add_dapp.clone(),
        },
    );

    let wallet = get_wallet(&mut context.banks_client, &wallet_account.pubkey()).await;
    // ensure that config policy updates are not locked
    assert!(!wallet.config_policy_update_locked);

    approve_n_of_n_multisig_op(&mut context, &multisig_op_account, vec![&approvers[0]]).await;

    utils::finalize_dapp_book_update(
        &mut context,
        wallet_account.pubkey(),
        multisig_op_account,
        add_dapp.clone(),
    )
    .await;

    assert_eq!(
        Slots::from_vec(vec![dapp_slot]),
        get_wallet(&mut context.banks_client, &wallet_account.pubkey())
            .await
            .dapp_book
    );

    // now remove it
    let remove_dapp = DAppBookUpdate {
        add_dapps: vec![],
        remove_dapps: vec![dapp_slot],
    };

    let remove_multisig_op_account = utils::init_dapp_book_update(
        &mut context,
        wallet_account.pubkey(),
        &assistant_account,
        remove_dapp.clone(),
    )
    .await
    .unwrap();

    assert_initialized_multisig_op(
        &get_multisig_op_data(&mut context.banks_client, remove_multisig_op_account).await,
        started_at,
        Duration::from_secs(3600),
        1,
        &vec![ApprovalDispositionRecord {
            approver: approvers[0].pubkey(),
            disposition: ApprovalDisposition::NONE,
        }],
        OperationDisposition::NONE,
        &MultisigOpParams::UpdateDAppBook {
            wallet_address: wallet_account.pubkey(),
            update: remove_dapp.clone(),
        },
    );

    approve_n_of_n_multisig_op(
        &mut context,
        &remove_multisig_op_account,
        vec![&approvers[0]],
    )
    .await;

    utils::finalize_dapp_book_update(
        &mut context,
        wallet_account.pubkey(),
        remove_multisig_op_account,
        remove_dapp.clone(),
    )
    .await;

    assert_eq!(
        Slots::new(),
        get_wallet(&mut context.banks_client, &wallet_account.pubkey())
            .await
            .dapp_book
    );
}
