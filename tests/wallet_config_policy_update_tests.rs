#![cfg(feature = "test-bpf")]
mod common;
pub use common::instructions::*;
pub use common::utils::*;

pub use common::utils;
use solana_program::instruction::InstructionError;
use solana_program::instruction::InstructionError::Custom;
use solana_program_test::tokio;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer as SdkSigner;
use std::time::{Duration, SystemTime};
use strike_wallet::error::WalletError;
use strike_wallet::instruction::WalletConfigPolicyUpdate;
use strike_wallet::model::multisig_op::{
    ApprovalDisposition, ApprovalDispositionRecord, MultisigOpParams, OperationDisposition,
};
use strike_wallet::model::wallet::{Approvers, Wallet};
use strike_wallet::utils::SlotId;

#[tokio::test]
async fn wallet_config_policy_update() {
    let started_at = SystemTime::now();
    let mut context = setup_test(30_000).await;

    let wallet_account = Keypair::new();
    let assistant_account = Keypair::new();

    let approvers = vec![Keypair::new(), Keypair::new(), Keypair::new()];
    let signers = vec![
        approvers[0].pubkey_as_signer(),
        approvers[1].pubkey_as_signer(),
        approvers[2].pubkey_as_signer(),
    ];

    utils::init_wallet(
        &mut context.banks_client,
        &context.payer,
        context.recent_blockhash,
        &context.program_id,
        &wallet_account,
        &assistant_account,
        Some(2),
        Some(vec![
            (SlotId::new(0), signers[0]),
            (SlotId::new(1), signers[1]),
            (SlotId::new(2), signers[2]),
        ]),
        Some(vec![
            (SlotId::new(0), signers[0]),
            (SlotId::new(1), signers[1]),
        ]),
        Some(Duration::from_secs(3600)),
        None,
    )
    .await
    .unwrap();

    let wallet = get_wallet(&mut context.banks_client, &wallet_account.pubkey()).await;
    assert!(!wallet.config_policy_update_locked);

    let update = WalletConfigPolicyUpdate {
        approvals_required_for_config: 1,
        approval_timeout_for_config: Duration::from_secs(7200),
        add_config_approvers: vec![(SlotId::new(2), signers[2])],
        remove_config_approvers: vec![(SlotId::new(0), signers[0])],
    };

    let multisig_op_account = utils::init_wallet_config_policy_update(
        &mut context,
        wallet_account.pubkey(),
        &assistant_account,
        &update,
    )
    .await
    .unwrap();

    assert_initialized_multisig_op(
        &get_multisig_op_data(&mut context.banks_client, multisig_op_account).await,
        started_at,
        Duration::from_secs(3600),
        2,
        &vec![
            ApprovalDispositionRecord {
                approver: approvers[0].pubkey(),
                disposition: ApprovalDisposition::NONE,
            },
            ApprovalDispositionRecord {
                approver: approvers[1].pubkey(),
                disposition: ApprovalDisposition::NONE,
            },
        ],
        OperationDisposition::NONE,
        &MultisigOpParams::UpdateWalletConfigPolicy {
            wallet_address: wallet_account.pubkey(),
            update: update.clone(),
        },
    );

    assert!(
        get_wallet(&mut context.banks_client, &wallet_account.pubkey())
            .await
            .config_policy_update_locked
    );

    approve_n_of_n_multisig_op(
        &mut context,
        &multisig_op_account,
        vec![&approvers[0], &approvers[1]],
    )
    .await;

    utils::finalize_wallet_config_policy_update(
        &mut context,
        wallet_account.pubkey(),
        multisig_op_account,
        &update.clone(),
    )
    .await;

    assert_eq!(
        Wallet {
            is_initialized: wallet.is_initialized,
            signers: wallet.signers,
            assistant: wallet.assistant,
            address_book: wallet.address_book,
            approvals_required_for_config: 1,
            approval_timeout_for_config: Duration::from_secs(7200),
            config_approvers: Approvers::from_enabled_vec(vec![SlotId::new(1), SlotId::new(2)]),
            balance_accounts: wallet.balance_accounts,
            config_policy_update_locked: false,
            dapp_book: wallet.dapp_book,
        },
        get_wallet(&mut context.banks_client, &wallet_account.pubkey()).await
    );
}

#[tokio::test]
async fn only_one_pending_wallet_config_policy_update_allowed_at_time() {
    let mut context = setup_test(30_000).await;

    let wallet_account = Keypair::new();
    let assistant_account = Keypair::new();

    let approvers = vec![Keypair::new(), Keypair::new(), Keypair::new()];
    let signers = vec![
        approvers[0].pubkey_as_signer(),
        approvers[1].pubkey_as_signer(),
        approvers[2].pubkey_as_signer(),
    ];

    utils::init_wallet(
        &mut context.banks_client,
        &context.payer,
        context.recent_blockhash,
        &context.program_id,
        &wallet_account,
        &assistant_account,
        Some(2),
        Some(vec![
            (SlotId::new(0), signers[0]),
            (SlotId::new(1), signers[1]),
            (SlotId::new(2), signers[2]),
        ]),
        Some(vec![
            (SlotId::new(0), signers[0]),
            (SlotId::new(1), signers[1]),
        ]),
        Some(Duration::from_secs(3600)),
        None,
    )
    .await
    .unwrap();

    let first_update = WalletConfigPolicyUpdate {
        approvals_required_for_config: 1,
        approval_timeout_for_config: Duration::from_secs(7200),
        add_config_approvers: vec![(SlotId::new(2), signers[2])],
        remove_config_approvers: vec![(SlotId::new(0), signers[0])],
    };

    let second_update = WalletConfigPolicyUpdate {
        approvals_required_for_config: 3,
        approval_timeout_for_config: Duration::from_secs(7200),
        add_config_approvers: vec![(SlotId::new(0), signers[0])],
        remove_config_approvers: vec![],
    };

    let multisig_op_account = utils::init_wallet_config_policy_update(
        &mut context,
        wallet_account.pubkey(),
        &assistant_account,
        &first_update,
    )
    .await
    .unwrap();

    // not allowed because first update is pending
    assert_instruction_error(
        utils::init_wallet_config_policy_update(
            &mut context,
            wallet_account.pubkey(),
            &assistant_account,
            &second_update,
        )
        .await,
        1,
        Custom(WalletError::ConcurrentOperationsNotAllowed as u32),
    );

    // deny and finalize
    deny_n_of_n_multisig_op(
        &mut context,
        &multisig_op_account,
        vec![&approvers[0], &approvers[1]],
    )
    .await;
    utils::finalize_wallet_config_policy_update(
        &mut context,
        wallet_account.pubkey(),
        multisig_op_account,
        &first_update.clone(),
    )
    .await;

    // allowed because first update is canceled
    let multisig_op_account = utils::init_wallet_config_policy_update(
        &mut context,
        wallet_account.pubkey(),
        &assistant_account,
        &first_update,
    )
    .await
    .unwrap();

    // not allowed because there is a new pending update
    assert_instruction_error(
        utils::init_wallet_config_policy_update(
            &mut context,
            wallet_account.pubkey(),
            &assistant_account,
            &second_update,
        )
        .await,
        1,
        Custom(WalletError::ConcurrentOperationsNotAllowed as u32),
    );

    // approve and finalize
    approve_n_of_n_multisig_op(
        &mut context,
        &multisig_op_account,
        vec![&approvers[0], &approvers[1]],
    )
    .await;
    utils::finalize_wallet_config_policy_update(
        &mut context,
        wallet_account.pubkey(),
        multisig_op_account,
        &first_update.clone(),
    )
    .await;

    // allowed because there is no pending update anymore
    utils::init_wallet_config_policy_update(
        &mut context,
        wallet_account.pubkey(),
        &assistant_account,
        &second_update,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn invalid_wallet_config_policy_updates() {
    let mut context = setup_test(30_000).await;

    let wallet_account = Keypair::new();
    let assistant_account = Keypair::new();

    let approvers = vec![Keypair::new(), Keypair::new(), Keypair::new()];
    let signers = vec![
        approvers[0].pubkey_as_signer(),
        approvers[1].pubkey_as_signer(),
        approvers[2].pubkey_as_signer(),
    ];

    utils::init_wallet(
        &mut context.banks_client,
        &context.payer,
        context.recent_blockhash,
        &context.program_id,
        &wallet_account,
        &assistant_account,
        Some(2),
        Some(vec![
            (SlotId::new(0), signers[0]),
            (SlotId::new(1), signers[1]),
        ]),
        Some(vec![
            (SlotId::new(0), signers[0]),
            (SlotId::new(1), signers[1]),
        ]),
        Some(Duration::from_secs(3600)),
        None,
    )
    .await
    .unwrap();

    // verify approvals required for config can't exceed configured approvers count
    assert_instruction_error(
        utils::init_wallet_config_policy_update(
            &mut context,
            wallet_account.pubkey(),
            &assistant_account,
            &WalletConfigPolicyUpdate {
                approvals_required_for_config: 3,
                approval_timeout_for_config: Duration::from_secs(3200),
                add_config_approvers: vec![],
                remove_config_approvers: vec![],
            },
        )
        .await,
        1,
        InstructionError::InvalidArgument,
    );

    // verify it's not allowed to add a config approver that is not configured as signer
    assert_instruction_error(
        utils::init_wallet_config_policy_update(
            &mut context,
            wallet_account.pubkey(),
            &assistant_account,
            &WalletConfigPolicyUpdate {
                approvals_required_for_config: 2,
                approval_timeout_for_config: Duration::from_secs(3200),
                add_config_approvers: vec![(SlotId::new(2), signers[2])],
                remove_config_approvers: vec![],
            },
        )
        .await,
        1,
        InstructionError::InvalidArgument,
    );

    // verify it's not allowed to add a config approver when provided slot value does not match the stored one
    assert_instruction_error(
        utils::init_wallet_config_policy_update(
            &mut context,
            wallet_account.pubkey(),
            &assistant_account,
            &WalletConfigPolicyUpdate {
                approvals_required_for_config: 2,
                approval_timeout_for_config: Duration::from_secs(3200),
                add_config_approvers: vec![(SlotId::new(0), signers[2])],
                remove_config_approvers: vec![],
            },
        )
        .await,
        1,
        InstructionError::InvalidArgument,
    );

    // verify it's not allowed to remove a config approver when provided slot value does not match the stored one
    assert_instruction_error(
        utils::init_wallet_config_policy_update(
            &mut context,
            wallet_account.pubkey(),
            &assistant_account,
            &WalletConfigPolicyUpdate {
                approvals_required_for_config: 2,
                approval_timeout_for_config: Duration::from_secs(3200),
                add_config_approvers: vec![],
                remove_config_approvers: vec![(SlotId::new(0), signers[2])],
            },
        )
        .await,
        1,
        InstructionError::InvalidArgument,
    );
}
