use crate::common::instructions;
use crate::common::instructions::{
    finalize_account_settings_update, finalize_balance_account_update, finalize_update_signer,
    finalize_wallet_config_policy_update_instruction, init_account_settings_update,
    init_balance_account_creation, init_balance_account_update, init_transfer, init_update_signer,
    init_wallet_config_policy_update_instruction, init_wallet_update, set_approval_disposition,
};
use arrayref::array_ref;
use itertools::Itertools;
use sha2::{Digest, Sha256};
use solana_program::instruction::{Instruction, InstructionError};
use solana_program::rent::Rent;
use solana_program::system_program;
use solana_program_test::{processor, ProgramTest};
use solana_sdk::account::ReadableAccount;
use solana_sdk::transaction::TransactionError;
use solana_sdk::transport;
use std::borrow::BorrowMut;
use std::collections::HashSet;
use std::fmt::Debug;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use strike_wallet::instruction::{
    BalanceAccountUpdate, DAppBookUpdate, WalletConfigPolicyUpdate, WalletUpdate,
};
use strike_wallet::model::address_book::{
    AddressBook, AddressBookEntry, AddressBookEntryNameHash, DAppBook,
};
use strike_wallet::model::balance_account::{BalanceAccountGuidHash, BalanceAccountNameHash};
use strike_wallet::model::multisig_op::{
    ApprovalDisposition, ApprovalDispositionRecord, BooleanSetting, MultisigOp, MultisigOpParams,
    OperationDisposition, SlotUpdateType, WrapDirection,
};
use strike_wallet::model::signer::Signer;
use strike_wallet::model::wallet::{Approvers, Signers};
use strike_wallet::utils::SlotId;
use uuid::Uuid;
use {
    solana_program::{program_pack::Pack, pubkey::Pubkey},
    solana_program_test::BanksClient,
    solana_sdk::{
        hash::Hash,
        signature::{Keypair, Signer as SdkSigner},
        system_instruction,
        transaction::Transaction,
        transport::TransportError,
    },
    strike_wallet::{model::wallet::Wallet, processor::Processor},
};

pub trait SignerKey {
    fn pubkey_as_signer(&self) -> Signer;
}
impl SignerKey for Keypair {
    fn pubkey_as_signer(&self) -> Signer {
        Signer::new(self.pubkey())
    }
}

pub trait ToSet<A> {
    fn to_set(&self) -> HashSet<A>;
}

impl<A: core::hash::Hash + Eq + Clone> ToSet<A> for Option<Vec<A>> {
    fn to_set(&self) -> HashSet<A> {
        match self {
            Some(items) => items.to_set(),
            None => HashSet::new(),
        }
    }
}

impl<A: core::hash::Hash + Eq + Clone> ToSet<A> for Vec<A> {
    fn to_set(&self) -> HashSet<A> {
        let mut set = HashSet::new();
        for item in self.iter() {
            set.insert(item.clone());
        }
        set
    }
}

pub struct TestContext {
    pub program_id: Pubkey,
    pub banks_client: BanksClient,
    pub rent: Rent,
    pub payer: Keypair,
    pub recent_blockhash: Hash,
}

pub async fn setup_test(max_compute_units: u64) -> TestContext {
    let program_id = Keypair::new().pubkey();
    let mut pt = ProgramTest::new("strike_wallet", program_id, processor!(Processor::process));
    pt.set_bpf_compute_max_units(max_compute_units);
    let (mut banks_client, payer, recent_blockhash) = pt.start().await;
    let rent = banks_client.get_rent().await.unwrap();

    TestContext {
        program_id,
        banks_client,
        rent,
        payer,
        recent_blockhash,
    }
}

pub fn create_program_owned_account_instruction(
    test_context: &TestContext,
    account_address: &Pubkey,
    space: usize,
) -> Instruction {
    system_instruction::create_account(
        &test_context.payer.pubkey(),
        &account_address,
        test_context.rent.minimum_balance(space),
        space as u64,
        &test_context.program_id,
    )
}

pub async fn init_multisig_op(
    test_context: &mut TestContext,
    multisig_op_account: Keypair,
    instruction: Instruction,
    assistant_account: &Keypair,
) -> transport::Result<()> {
    test_context
        .banks_client
        .process_transaction(Transaction::new_signed_with_payer(
            &[
                create_program_owned_account_instruction(
                    &test_context,
                    &multisig_op_account.pubkey(),
                    MultisigOp::LEN,
                ),
                instruction,
            ],
            Some(&test_context.payer.pubkey()),
            &[&test_context.payer, &multisig_op_account, assistant_account],
            test_context.recent_blockhash,
        ))
        .await
}

pub async fn finalize_multisig_op(
    test_context: &mut TestContext,
    multisig_op_account: Pubkey,
    instruction: Instruction,
) {
    let starting_rent_collector_balance = test_context
        .banks_client
        .get_balance(test_context.payer.pubkey())
        .await
        .unwrap();

    let op_account_balance = test_context
        .banks_client
        .get_balance(multisig_op_account)
        .await
        .unwrap();

    test_context
        .banks_client
        .process_transaction(Transaction::new_signed_with_payer(
            &[instruction],
            Some(&test_context.payer.pubkey()),
            &[&test_context.payer],
            test_context.recent_blockhash,
        ))
        .await
        .unwrap();

    // verify the multisig op account is closed
    assert!(test_context
        .banks_client
        .get_account(multisig_op_account)
        .await
        .unwrap()
        .is_none());

    // and that the remaining balance went to the rent collector (less the 5000 in fees for the finalize)
    let ending_rent_collector_balance = test_context
        .banks_client
        .get_balance(test_context.payer.pubkey())
        .await
        .unwrap();

    assert_eq!(
        starting_rent_collector_balance + op_account_balance - 5000,
        ending_rent_collector_balance
    );
}

pub async fn get_multisig_op_data(
    banks_client: &mut BanksClient,
    account_address: Pubkey,
) -> MultisigOp {
    return MultisigOp::unpack_from_slice(
        banks_client
            .get_account(account_address)
            .await
            .unwrap()
            .unwrap()
            .data(),
    )
    .unwrap();
}

pub async fn init_wallet_config_policy_update(
    test_context: &mut TestContext,
    wallet_account: Pubkey,
    assistant: &Keypair,
    update: &WalletConfigPolicyUpdate,
) -> Result<Pubkey, TransportError> {
    let multisig_op_keypair = Keypair::new();
    let multisig_op_pubkey = multisig_op_keypair.pubkey();

    let instruction = init_wallet_config_policy_update_instruction(
        test_context.program_id,
        wallet_account,
        multisig_op_pubkey,
        assistant.pubkey(),
        update,
    );

    init_multisig_op(test_context, multisig_op_keypair, instruction, assistant)
        .await
        .map(|_| multisig_op_pubkey)
}

pub async fn finalize_wallet_config_policy_update(
    test_context: &mut TestContext,
    wallet_account: Pubkey,
    multisig_op_account: Pubkey,
    update: &WalletConfigPolicyUpdate,
) {
    finalize_multisig_op(
        test_context,
        multisig_op_account,
        finalize_wallet_config_policy_update_instruction(
            test_context.program_id,
            wallet_account,
            multisig_op_account,
            test_context.payer.pubkey(),
            update,
        ),
    )
    .await;
}

pub fn assert_instruction_error<R: Debug>(
    res: Result<R, TransportError>,
    expected_instruction_index: u8,
    expected_error: InstructionError,
) {
    assert_eq!(
        res.unwrap_err().unwrap(),
        TransactionError::InstructionError(expected_instruction_index, expected_error)
    );
}

pub async fn init_wallet(
    banks_client: &mut BanksClient,
    payer: &Keypair,
    recent_blockhash: Hash,
    program_id: &Pubkey,
    wallet_account: &Keypair,
    assistant_account: &Keypair,
    approvals_required_for_config: Option<u8>,
    signers: Option<Vec<(SlotId<Signer>, Signer)>>,
    config_approvers: Option<Vec<(SlotId<Signer>, Signer)>>,
    approval_timeout_for_config: Option<Duration>,
    address_book: Option<Vec<(SlotId<AddressBookEntry>, AddressBookEntry)>>,
) -> Result<(), TransportError> {
    let rent = banks_client.get_rent().await.unwrap();
    let program_rent = rent.minimum_balance(Wallet::LEN);

    let transaction = Transaction::new_signed_with_payer(
        &[
            system_instruction::create_account(
                &payer.pubkey(),
                &wallet_account.pubkey(),
                program_rent,
                Wallet::LEN as u64,
                &program_id,
            ),
            instructions::init_wallet(
                &program_id,
                &wallet_account.pubkey(),
                &assistant_account.pubkey(),
                signers.unwrap_or(Vec::new()),
                config_approvers.unwrap_or(Vec::new()),
                approvals_required_for_config.unwrap_or(0),
                approval_timeout_for_config.unwrap_or(Duration::from_secs(0)),
                address_book.unwrap_or(Vec::new()),
            ),
        ],
        Some(&payer.pubkey()),
        &[payer, wallet_account, assistant_account],
        recent_blockhash,
    );
    banks_client.process_transaction(transaction).await?;
    Ok(())
}

pub struct WalletUpdateContext {
    pub payer: Keypair,
    pub program_id: Pubkey,
    pub banks_client: BanksClient,
    pub wallet_account: Keypair,
    pub multisig_op_account: Keypair,
    pub approvers: Vec<Keypair>,
    pub recent_blockhash: Hash,
    pub expected_update: WalletUpdate,
    pub params_hash: Hash,
    pub expected_state_after_update: Wallet,
    pub assistant_account: Keypair,
}

pub async fn setup_wallet_update_test() -> WalletUpdateContext {
    let mut test_context = setup_test(30_000).await;

    let wallet_account = Keypair::new();
    let multisig_op_account = Keypair::new();
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
    init_wallet(
        &mut test_context.banks_client,
        &test_context.payer,
        test_context.recent_blockhash,
        &test_context.program_id,
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

    let wallet = get_wallet(&mut test_context.banks_client, &wallet_account.pubkey()).await;

    // now initialize an update
    let init_transaction = Transaction::new_signed_with_payer(
        &[
            create_program_owned_account_instruction(
                &mut test_context,
                &multisig_op_account.pubkey(),
                MultisigOp::LEN,
            ),
            init_wallet_update(
                &test_context.program_id,
                &wallet_account.pubkey(),
                &multisig_op_account.pubkey(),
                &assistant_account.pubkey(),
                2,
                Duration::from_secs(7200),
                vec![(SlotId::new(2), signers[2])],
                vec![(SlotId::new(0), signers[0])],
                vec![(SlotId::new(2), signers[2])],
                vec![(SlotId::new(0), signers[0])],
                vec![(SlotId::new(0), new_address_book_entry)],
                vec![(SlotId::new(0), address_book_entry)],
            ),
        ],
        Some(&test_context.payer.pubkey()),
        &[
            &test_context.payer,
            &multisig_op_account,
            &assistant_account,
        ],
        test_context.recent_blockhash,
    );
    test_context
        .banks_client
        .process_transaction(init_transaction)
        .await
        .unwrap();

    let expected_update = WalletUpdate {
        approvals_required_for_config: 2,
        approval_timeout_for_config: Duration::from_secs(7200),
        add_signers: vec![(SlotId::new(2), signers[2])],
        remove_signers: vec![(SlotId::new(0), signers[0])],
        add_config_approvers: vec![(SlotId::new(2), signers[2])],
        remove_config_approvers: vec![(SlotId::new(0), signers[0])],
        add_address_book_entries: vec![(SlotId::new(0), new_address_book_entry)],
        remove_address_book_entries: vec![(SlotId::new(0), address_book_entry)],
    };

    let multisig_op =
        get_multisig_op_data(&mut test_context.banks_client, multisig_op_account.pubkey()).await;

    assert!(multisig_op.is_initialized);

    WalletUpdateContext {
        payer: test_context.payer,
        program_id: test_context.program_id,
        banks_client: test_context.banks_client,
        wallet_account,
        multisig_op_account,
        approvers,
        recent_blockhash: test_context.recent_blockhash,
        expected_update,
        params_hash: multisig_op.params_hash,
        expected_state_after_update: Wallet {
            is_initialized: true,
            signers: Signers::from_vec(vec![
                (SlotId::new(1), signers[1]),
                (SlotId::new(2), signers[2]),
            ]),
            assistant: wallet.assistant,
            address_book: AddressBook::from_vec(vec![(SlotId::new(0), new_address_book_entry)]),
            approvals_required_for_config: 2,
            approval_timeout_for_config: Duration::from_secs(7200),
            config_approvers: Approvers::from_enabled_vec(vec![SlotId::new(1), SlotId::new(2)]),
            balance_accounts: wallet.balance_accounts,
            config_policy_update_locked: false,
            dapp_book: DAppBook::from_vec(vec![]),
        },
        assistant_account,
    }
}

pub async fn update_signer(
    context: &mut WalletUpdateContext,
    slot_update_type: SlotUpdateType,
    slot_id: usize,
    signer: Signer,
    expected_signers: Option<Signers>,
    expected_error: Option<InstructionError>,
) {
    let rent = context.banks_client.get_rent().await.unwrap();
    let multisig_op_rent = rent.minimum_balance(MultisigOp::LEN);
    let multisig_op_account = Keypair::new();
    let init_transaction = Transaction::new_signed_with_payer(
        &[
            system_instruction::create_account(
                &context.payer.pubkey(),
                &multisig_op_account.pubkey(),
                multisig_op_rent,
                MultisigOp::LEN as u64,
                &context.program_id,
            ),
            init_update_signer(
                &context.program_id,
                &context.wallet_account.pubkey(),
                &multisig_op_account.pubkey(),
                &context.assistant_account.pubkey(),
                slot_update_type,
                SlotId::new(slot_id),
                signer,
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
    match expected_error {
        None => context
            .banks_client
            .process_transaction(init_transaction)
            .await
            .unwrap(),
        Some(error) => {
            assert_eq!(
                context
                    .banks_client
                    .process_transaction(init_transaction)
                    .await
                    .unwrap_err()
                    .unwrap(),
                TransactionError::InstructionError(1, error),
            );
            return;
        }
    }

    // verify the multisig op account data
    let multisig_op = MultisigOp::unpack_from_slice(
        context
            .banks_client
            .get_account(multisig_op_account.pubkey())
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

    assert_eq!(
        multisig_op.params_hash,
        MultisigOpParams::UpdateSigner {
            wallet_address: context.wallet_account.pubkey(),
            slot_update_type,
            slot_id: SlotId::new(slot_id),
            signer
        }
        .hash()
    );

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

    // verify the multisig op account data
    let multisig_op = MultisigOp::unpack_from_slice(
        context
            .banks_client
            .get_account(multisig_op_account.pubkey())
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
        &[finalize_update_signer(
            &context.program_id,
            &context.wallet_account.pubkey(),
            &multisig_op_account.pubkey(),
            &context.payer.pubkey(),
            slot_update_type,
            SlotId::new(slot_id),
            signer,
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
        .get_balance(multisig_op_account.pubkey())
        .await
        .unwrap();
    context
        .banks_client
        .process_transaction(finalize_transaction)
        .await
        .unwrap();

    // verify the config has been updated
    let wallet = get_wallet(&mut context.banks_client, &context.wallet_account.pubkey()).await;
    assert_eq!(expected_signers.unwrap(), wallet.signers);

    // verify the multisig op account is closed
    assert!(context
        .banks_client
        .get_account(multisig_op_account.pubkey())
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

pub async fn account_settings_update(
    context: &mut BalanceAccountTestContext,
    whitelist_status: Option<BooleanSetting>,
    dapps_enabled: Option<BooleanSetting>,
    expected_error: Option<InstructionError>,
) {
    let rent = context.banks_client.get_rent().await.unwrap();
    let multisig_op_rent = rent.minimum_balance(MultisigOp::LEN);
    let multisig_op_account = Keypair::new();
    let init_transaction = Transaction::new_signed_with_payer(
        &[
            system_instruction::create_account(
                &context.payer.pubkey(),
                &multisig_op_account.pubkey(),
                multisig_op_rent,
                MultisigOp::LEN as u64,
                &context.program_id,
            ),
            init_account_settings_update(
                &context.program_id,
                &context.wallet_account.pubkey(),
                &multisig_op_account.pubkey(),
                &context.assistant_account.pubkey(),
                context.balance_account_guid_hash,
                whitelist_status,
                dapps_enabled,
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
    match expected_error {
        None => context
            .banks_client
            .process_transaction(init_transaction)
            .await
            .unwrap(),
        Some(error) => {
            assert_eq!(
                context
                    .banks_client
                    .process_transaction(init_transaction)
                    .await
                    .unwrap_err()
                    .unwrap(),
                TransactionError::InstructionError(1, error),
            );
            return;
        }
    }

    // verify the multisig op account data
    let multisig_op = MultisigOp::unpack_from_slice(
        context
            .banks_client
            .get_account(multisig_op_account.pubkey())
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

    assert_eq!(
        multisig_op.params_hash,
        MultisigOpParams::UpdateAccountSettings {
            wallet_address: context.wallet_account.pubkey(),
            account_guid_hash: context.balance_account_guid_hash,
            whitelist_enabled: whitelist_status,
            dapps_enabled,
        }
        .hash()
    );

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

    // verify the multisig op account data
    let multisig_op = MultisigOp::unpack_from_slice(
        context
            .banks_client
            .get_account(multisig_op_account.pubkey())
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
        &[finalize_account_settings_update(
            &context.program_id,
            &context.wallet_account.pubkey(),
            &multisig_op_account.pubkey(),
            &context.payer.pubkey(),
            context.balance_account_guid_hash,
            whitelist_status,
            dapps_enabled,
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
        .get_balance(multisig_op_account.pubkey())
        .await
        .unwrap();
    context
        .banks_client
        .process_transaction(finalize_transaction)
        .await
        .unwrap();

    // verify the multisig op account is closed
    assert!(context
        .banks_client
        .get_account(multisig_op_account.pubkey())
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

pub async fn init_dapp_book_update(
    test_context: &mut TestContext,
    wallet_account: Pubkey,
    assistant: &Keypair,
    update: DAppBookUpdate,
) -> Result<Pubkey, TransportError> {
    let multisig_op_keypair = Keypair::new();
    let multisig_op_pubkey = multisig_op_keypair.pubkey();

    let instruction = instructions::init_dapp_book_update(
        &test_context.program_id,
        &wallet_account,
        &multisig_op_pubkey,
        &assistant.pubkey(),
        update,
    );

    init_multisig_op(test_context, multisig_op_keypair, instruction, assistant)
        .await
        .map(|_| multisig_op_pubkey)
}

pub async fn finalize_dapp_book_update(
    test_context: &mut TestContext,
    wallet_account: Pubkey,
    multisig_op_account: Pubkey,
    update: DAppBookUpdate,
) {
    finalize_multisig_op(
        test_context,
        multisig_op_account,
        instructions::finalize_dapp_book_update(
            &test_context.program_id,
            &wallet_account,
            &multisig_op_account,
            &test_context.payer.pubkey(),
            update,
        ),
    )
    .await;
}

pub async fn verify_whitelist_status(
    context: &mut BalanceAccountTestContext,
    expected_status: BooleanSetting,
    expected_whitelist_count: usize,
) {
    let wallet = get_wallet(&mut context.banks_client, &context.wallet_account.pubkey()).await;
    let account = wallet
        .get_balance_account(&context.balance_account_guid_hash)
        .unwrap();

    assert_eq!(account.whitelist_enabled, expected_status);
    assert_eq!(
        account.allowed_destinations.count_enabled(),
        expected_whitelist_count
    );
}

pub async fn verify_dapps_enabled(
    context: &mut BalanceAccountTestContext,
    expected_enabled: BooleanSetting,
) {
    let wallet = get_wallet(&mut context.banks_client, &context.wallet_account.pubkey()).await;
    let account = wallet
        .get_balance_account(&context.balance_account_guid_hash)
        .unwrap();

    assert_eq!(account.dapps_enabled, expected_enabled);
}

pub async fn approve_or_deny_n_of_n_multisig_op(
    banks_client: &mut BanksClient,
    program_id: &Pubkey,
    multisig_op_account: &Pubkey,
    approvers: Vec<&Keypair>,
    payer: &Keypair,
    recent_blockhash: Hash,
    disposition: ApprovalDisposition,
    expected_operation_disposition: OperationDisposition,
) {
    let params_hash = get_operation_hash(banks_client.borrow_mut(), *multisig_op_account).await;

    // approve the config change
    for approver in approvers.iter() {
        let approve_transaction = Transaction::new_signed_with_payer(
            &[set_approval_disposition(
                program_id,
                multisig_op_account,
                &approver.pubkey(),
                disposition,
                params_hash,
            )],
            Some(&payer.pubkey()),
            &[payer, approver],
            recent_blockhash,
        );
        banks_client
            .process_transaction(approve_transaction)
            .await
            .unwrap();
    }

    // verify the disposition was recorded in the multisig op account
    let multisig_op = MultisigOp::unpack_from_slice(
        banks_client
            .get_account(*multisig_op_account)
            .await
            .unwrap()
            .unwrap()
            .data(),
    )
    .unwrap();
    assert_eq!(
        multisig_op.disposition_records.to_set(),
        approvers
            .iter()
            .map(|approver| ApprovalDispositionRecord {
                approver: approver.pubkey(),
                disposition,
            })
            .collect_vec()
            .to_set()
    );
    assert_eq!(
        multisig_op.operation_disposition,
        expected_operation_disposition
    )
}

pub async fn approve_n_of_n_multisig_op(
    test_context: &mut TestContext,
    multisig_op_account: &Pubkey,
    approvers: Vec<&Keypair>,
) {
    approve_or_deny_n_of_n_multisig_op(
        &mut test_context.banks_client,
        &test_context.program_id,
        &multisig_op_account,
        approvers,
        &test_context.payer,
        test_context.recent_blockhash,
        ApprovalDisposition::APPROVE,
        OperationDisposition::APPROVED,
    )
    .await;
}

pub async fn deny_n_of_n_multisig_op(
    test_context: &mut TestContext,
    multisig_op_account: &Pubkey,
    approvers: Vec<&Keypair>,
) {
    approve_or_deny_n_of_n_multisig_op(
        &mut test_context.banks_client,
        &test_context.program_id,
        &multisig_op_account,
        approvers,
        &test_context.payer,
        test_context.recent_blockhash,
        ApprovalDisposition::DENY,
        OperationDisposition::DENIED,
    )
    .await;
}

pub async fn approve_or_deny_1_of_2_multisig_op(
    banks_client: &mut BanksClient,
    program_id: &Pubkey,
    multisig_op_account: &Pubkey,
    approver: &Keypair,
    payer: &Keypair,
    other_approver: &Pubkey,
    recent_blockhash: Hash,
    disposition: ApprovalDisposition,
) {
    let params_hash = get_operation_hash(banks_client.borrow_mut(), *multisig_op_account).await;

    // approve the config change
    let approve_transaction = Transaction::new_signed_with_payer(
        &[set_approval_disposition(
            program_id,
            multisig_op_account,
            &approver.pubkey(),
            disposition,
            params_hash,
        )],
        Some(&payer.pubkey()),
        &[payer, approver],
        recent_blockhash,
    );
    banks_client
        .process_transaction(approve_transaction)
        .await
        .unwrap();

    // verify the disposition was recorded in the multisig op account
    let multisig_op = MultisigOp::unpack_from_slice(
        banks_client
            .get_account(*multisig_op_account)
            .await
            .unwrap()
            .unwrap()
            .data(),
    )
    .unwrap();
    assert_eq!(
        multisig_op.disposition_records.to_set(),
        HashSet::from([
            ApprovalDispositionRecord {
                approver: approver.pubkey(),
                disposition,
            },
            ApprovalDispositionRecord {
                approver: *other_approver,
                disposition: ApprovalDisposition::NONE,
            },
        ])
    );
}

pub fn hash_of(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let hash_output = hasher.finalize();
    *array_ref![hash_output, 0, 32]
}

pub struct BalanceAccountTestContext {
    pub payer: Keypair,
    pub program_id: Pubkey,
    pub banks_client: BanksClient,
    pub rent: Rent,
    pub wallet_account: Keypair,
    pub multisig_op_account: Keypair,
    pub assistant_account: Keypair,
    pub approvers: Vec<Keypair>,
    pub recent_blockhash: Hash,
    pub expected_update: BalanceAccountUpdate,
    pub balance_account_name_hash: BalanceAccountNameHash,
    pub balance_account_guid_hash: BalanceAccountGuidHash,
    pub destination_name_hash: AddressBookEntryNameHash,
    pub allowed_destination: AddressBookEntry,
    pub destination: Keypair,
    pub params_hash: Hash,
    pub allowed_dapp: AddressBookEntry,
}

impl BalanceAccountTestContext {
    fn to_test_context(&self) -> TestContext {
        let new_payer = Keypair::from_bytes(&self.payer.to_bytes()[..]).unwrap();
        TestContext {
            program_id: self.program_id,
            banks_client: self.banks_client.clone(),
            rent: self.rent,
            payer: new_payer,
            recent_blockhash: self.recent_blockhash,
        }
    }
}

pub async fn setup_balance_account_tests(
    bpf_compute_max_units: Option<u64>,
    add_extra_transfer_approver: bool,
) -> BalanceAccountTestContext {
    let program_id = Keypair::new().pubkey();
    let mut pt = ProgramTest::new("strike_wallet", program_id, processor!(Processor::process));
    pt.set_bpf_compute_max_units(bpf_compute_max_units.unwrap_or(25_000));
    let (mut banks_client, payer, recent_blockhash) = pt.start().await;
    let wallet_account = Keypair::new();
    let multisig_op_account = Keypair::new();
    let assistant_account = Keypair::new();

    let approvers = vec![Keypair::new(), Keypair::new(), Keypair::new()];

    let destination = Keypair::new();
    let addr_book_entry = AddressBookEntry {
        address: destination.pubkey(),
        name_hash: AddressBookEntryNameHash::new(&hash_of(b"Destination 1 Name")),
    };
    let addr_book_entry2 = AddressBookEntry {
        address: Keypair::new().pubkey(),
        name_hash: AddressBookEntryNameHash::new(&hash_of(b"Destination 2 Name")),
    };
    let allowed_dapp = AddressBookEntry {
        address: Keypair::new().pubkey(),
        name_hash: AddressBookEntryNameHash::new(&hash_of(b"DApp Name")),
    };

    // first initialize the wallet
    init_wallet(
        &mut banks_client,
        &payer,
        recent_blockhash,
        &program_id,
        &wallet_account,
        &assistant_account,
        Some(1),
        Some(vec![
            (SlotId::new(0), approvers[0].pubkey_as_signer()),
            (SlotId::new(1), approvers[1].pubkey_as_signer()),
            (SlotId::new(2), approvers[2].pubkey_as_signer()),
        ]),
        Some(vec![
            (SlotId::new(0), approvers[0].pubkey_as_signer()),
            (SlotId::new(1), approvers[1].pubkey_as_signer()),
        ]),
        Some(Duration::from_secs(3600)),
        Some(vec![
            (SlotId::new(0), addr_book_entry),
            (SlotId::new(1), addr_book_entry2),
        ]),
    )
    .await
    .unwrap();

    // now initialize balance account creation
    let rent = banks_client.get_rent().await.unwrap();
    let multisig_account_rent = rent.minimum_balance(MultisigOp::LEN);
    let balance_account_guid_hash =
        BalanceAccountGuidHash::new(&hash_of(Uuid::new_v4().as_bytes()));
    let balance_account_name_hash = BalanceAccountNameHash::new(&hash_of(b"Account Name"));
    let approval_timeout_for_transfer = Duration::from_secs(120);

    let mut transfer_approvers = vec![
        (SlotId::new(0), approvers[0].pubkey_as_signer()),
        (SlotId::new(1), approvers[1].pubkey_as_signer()),
    ];
    if add_extra_transfer_approver {
        transfer_approvers.append(&mut vec![(SlotId::new(2), approvers[2].pubkey_as_signer())])
    }

    let init_transaction = Transaction::new_signed_with_payer(
        &[
            system_instruction::create_account(
                &payer.pubkey(),
                &multisig_op_account.pubkey(),
                multisig_account_rent,
                MultisigOp::LEN as u64,
                &program_id,
            ),
            init_balance_account_creation(
                &program_id,
                &wallet_account.pubkey(),
                &multisig_op_account.pubkey(),
                &assistant_account.pubkey(),
                balance_account_guid_hash,
                balance_account_name_hash,
                2,
                approval_timeout_for_transfer,
                transfer_approvers.clone(),
                vec![],
            ),
        ],
        Some(&payer.pubkey()),
        &[&payer, &multisig_op_account, &assistant_account],
        recent_blockhash,
    );
    banks_client
        .process_transaction(init_transaction)
        .await
        .unwrap();

    // verify the multisig op account data
    let multisig_op = MultisigOp::unpack_from_slice(
        banks_client
            .get_account(multisig_op_account.pubkey())
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
                approver: approvers[0].pubkey(),
                disposition: ApprovalDisposition::NONE,
            },
            ApprovalDispositionRecord {
                approver: approvers[1].pubkey(),
                disposition: ApprovalDisposition::NONE,
            },
        ])
    );
    assert_eq!(multisig_op.dispositions_required, 1);

    let expected_update = BalanceAccountUpdate {
        name_hash: balance_account_name_hash,
        approvals_required_for_transfer: 2,
        approval_timeout_for_transfer,
        add_transfer_approvers: transfer_approvers.clone(),
        remove_transfer_approvers: vec![],
        add_allowed_destinations: vec![],
        remove_allowed_destinations: vec![],
    };

    assert_eq!(
        multisig_op.params_hash,
        MultisigOpParams::CreateBalanceAccount {
            wallet_address: wallet_account.pubkey(),
            account_guid_hash: balance_account_guid_hash,
            update: expected_update.clone(),
        }
        .hash()
    );

    BalanceAccountTestContext {
        payer,
        program_id,
        banks_client,
        rent,
        wallet_account,
        multisig_op_account,
        assistant_account,
        approvers,
        recent_blockhash,
        expected_update,
        balance_account_name_hash,
        balance_account_guid_hash,
        destination_name_hash: addr_book_entry.name_hash,
        allowed_destination: addr_book_entry,
        destination,
        params_hash: multisig_op.params_hash,
        allowed_dapp,
    }
}

pub async fn get_operation_hash(banks_client: &mut BanksClient, op_address: Pubkey) -> Hash {
    let multisig_op = MultisigOp::unpack_from_slice(
        banks_client
            .get_account(op_address)
            .await
            .unwrap()
            .unwrap()
            .data(),
    )
    .unwrap();

    multisig_op.params_hash
}

pub async fn setup_create_balance_account_failure_tests(
    bpf_compute_max_units: Option<u64>,
    approvals_required_for_transfer: u8,
    approval_timeout_for_transfer: Duration,
    transfer_approvers: Vec<Pubkey>,
) -> TransactionError {
    let program_id = Keypair::new().pubkey();
    let mut pt = ProgramTest::new("strike_wallet", program_id, processor!(Processor::process));
    pt.set_bpf_compute_max_units(bpf_compute_max_units.unwrap_or(25_000));
    let (mut banks_client, payer, recent_blockhash) = pt.start().await;
    let wallet_account = Keypair::new();
    let multisig_op_account = Keypair::new();
    let assistant_account = Keypair::new();

    let approvers = vec![Keypair::new(), Keypair::new(), Keypair::new()];

    // add given transfer approvers to signers
    let mut signers = transfer_approvers
        .iter()
        .enumerate()
        .map(|(i, pk)| (SlotId::new(i), Signer::new(*pk)))
        .collect_vec();
    // add a couple random signers to ensure init wallet has a non-zero signers
    // vec in case the given transfer_approvers vec has insufficient length.
    approvers
        .iter()
        .for_each(|kp| signers.push((SlotId::new(signers.len()), kp.pubkey_as_signer())));

    // take the first two signers as config approvers
    let config_approvers = signers[..2].to_vec();

    // first initialize the wallet
    init_wallet(
        &mut banks_client,
        &payer,
        recent_blockhash,
        &program_id,
        &wallet_account,
        &assistant_account,
        Some(1),
        Some(signers),
        Some(config_approvers),
        Some(Duration::from_secs(3600)),
        Some(vec![]),
    )
    .await
    .unwrap();

    // now initialize a balance account creation
    let rent = banks_client.get_rent().await.unwrap();
    let multisig_account_rent = rent.minimum_balance(MultisigOp::LEN);
    let balance_account_guid_hash =
        BalanceAccountGuidHash::new(&hash_of(Uuid::new_v4().as_bytes()));
    let balance_account_name_hash = BalanceAccountNameHash::new(&hash_of(b"Account Name"));

    let init_transaction = Transaction::new_signed_with_payer(
        &[
            system_instruction::create_account(
                &payer.pubkey(),
                &multisig_op_account.pubkey(),
                multisig_account_rent,
                MultisigOp::LEN as u64,
                &program_id,
            ),
            init_balance_account_creation(
                &program_id,
                &wallet_account.pubkey(),
                &multisig_op_account.pubkey(),
                &assistant_account.pubkey(),
                balance_account_guid_hash,
                balance_account_name_hash,
                approvals_required_for_transfer,
                approval_timeout_for_transfer,
                transfer_approvers
                    .iter()
                    .enumerate()
                    .map(|(i, pk)| (SlotId::new(i), Signer::new(*pk)))
                    .collect_vec(),
                vec![],
            ),
        ],
        Some(&payer.pubkey()),
        &[&payer, &multisig_op_account, &assistant_account],
        recent_blockhash,
    );
    banks_client
        .process_transaction(init_transaction)
        .await
        .unwrap_err()
        .unwrap()
}

pub async fn finalize_balance_account_creation(context: &mut BalanceAccountTestContext) {
    let finalize_transaction = Transaction::new_signed_with_payer(
        &[instructions::finalize_balance_account_creation(
            &context.program_id,
            &context.wallet_account.pubkey(),
            &context.multisig_op_account.pubkey(),
            &context.payer.pubkey(),
            context.balance_account_guid_hash,
            context.expected_update.clone(),
        )],
        Some(&context.payer.pubkey()),
        &[&context.payer],
        context.recent_blockhash,
    );
    context
        .banks_client
        .process_transaction(finalize_transaction)
        .await
        .unwrap();
}

pub async fn setup_balance_account_tests_and_finalize(
    bpf_compute_max_units: Option<u64>,
) -> (BalanceAccountTestContext, Pubkey) {
    let mut context = setup_balance_account_tests(bpf_compute_max_units, false).await;

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

    finalize_balance_account_creation(context.borrow_mut()).await;
    let (source_account, _) = Pubkey::find_program_address(
        &[&context.balance_account_guid_hash.to_bytes()],
        &context.program_id,
    );

    // add allowed dapp
    let mut test_context = context.to_test_context();
    let update = DAppBookUpdate {
        add_dapps: vec![(SlotId::new(0), context.allowed_dapp)],
        remove_dapps: vec![],
    };

    let multisig_op_account = init_dapp_book_update(
        &mut test_context,
        context.wallet_account.pubkey(),
        &context.assistant_account,
        update.clone(),
    )
    .await
    .unwrap();

    approve_n_of_n_multisig_op(
        &mut test_context,
        &multisig_op_account,
        vec![&context.approvers[0], &context.approvers[1]],
    )
    .await;

    finalize_dapp_book_update(
        &mut test_context,
        context.wallet_account.pubkey(),
        multisig_op_account,
        update.clone(),
    )
    .await;

    (context, source_account)
}

pub async fn setup_transfer_test(
    context: &mut BalanceAccountTestContext,
    balance_account: &Pubkey,
    token_mint: Option<&Pubkey>,
    amount: Option<u64>,
) -> (Keypair, transport::Result<()>) {
    let rent = context.banks_client.get_rent().await.unwrap();
    let multisig_account_rent = rent.minimum_balance(MultisigOp::LEN);
    let multisig_op_account = Keypair::new();
    let initialized_at = SystemTime::now();

    let result = context
        .banks_client
        .process_transaction(Transaction::new_signed_with_payer(
            &[
                system_instruction::create_account(
                    &context.payer.pubkey(),
                    &multisig_op_account.pubkey(),
                    multisig_account_rent,
                    MultisigOp::LEN as u64,
                    &context.program_id,
                ),
                init_transfer(
                    &context.program_id,
                    &context.wallet_account.pubkey(),
                    &multisig_op_account.pubkey(),
                    &context.assistant_account.pubkey(),
                    &balance_account,
                    &context.destination.pubkey(),
                    context.balance_account_guid_hash,
                    amount.unwrap_or(123),
                    context.destination_name_hash,
                    token_mint.unwrap_or(&system_program::id()),
                    &context.payer.pubkey(),
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
        .await;

    if result.is_ok() {
        assert_multisig_op_timestamps(
            &get_multisig_op_data(&mut context.banks_client, multisig_op_account.pubkey()).await,
            initialized_at,
            Duration::from_secs(120),
        );
    }

    (multisig_op_account, result)
}

pub async fn modify_whitelist(
    context: &mut BalanceAccountTestContext,
    destinations_to_add: Vec<(SlotId<AddressBookEntry>, AddressBookEntry)>,
    destinations_to_remove: Vec<(SlotId<AddressBookEntry>, AddressBookEntry)>,
    expected_error: Option<InstructionError>,
) {
    // add a whitelisted destination
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
                Duration::from_secs(120),
                vec![],
                vec![],
                destinations_to_add.clone(),
                destinations_to_remove.clone(),
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
    match expected_error {
        None => context
            .banks_client
            .process_transaction(balance_account_update_transaction)
            .await
            .unwrap(),
        Some(error) => {
            assert_eq!(
                context
                    .banks_client
                    .process_transaction(balance_account_update_transaction)
                    .await
                    .unwrap_err()
                    .unwrap(),
                TransactionError::InstructionError(1, error),
            );
            return;
        }
    }

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
        approval_timeout_for_transfer: Duration::from_secs(120),
        add_transfer_approvers: vec![],
        remove_transfer_approvers: vec![],
        add_allowed_destinations: destinations_to_add,
        remove_allowed_destinations: destinations_to_remove,
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
}

pub struct SPLTestContext {
    pub mint: Keypair,
    pub mint_authority: Keypair,
    pub source_token_address: Pubkey,
    pub destination_token_address: Pubkey,
}

pub async fn setup_spl_transfer_test(
    context: &mut BalanceAccountTestContext,
    source_account: &Pubkey,
    fund_source_account_to_pay_for_destination_token_account: bool,
) -> SPLTestContext {
    let rent = context.banks_client.get_rent().await.unwrap();
    let mint_account_rent = rent.minimum_balance(spl_token::state::Mint::LEN);
    let mint = Keypair::new();
    let mint_authority = Keypair::new();
    let source_token_address =
        spl_associated_token_account::get_associated_token_address(source_account, &mint.pubkey());
    let destination_token_address = spl_associated_token_account::get_associated_token_address(
        &context.destination.pubkey(),
        &mint.pubkey(),
    );

    context
        .banks_client
        .process_transaction(Transaction::new_signed_with_payer(
            &[
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
                    source_account,
                    &mint.pubkey(),
                ),
                system_instruction::create_account(
                    &context.payer.pubkey(),
                    &context.destination.pubkey(),
                    0,
                    0,
                    &system_program::id(),
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
            ],
            Some(&context.payer.pubkey()),
            &[&context.payer, &mint, &mint_authority, &context.destination],
            context.recent_blockhash,
        ))
        .await
        .unwrap();

    if fund_source_account_to_pay_for_destination_token_account {
        // transfer enough balance from fee payer to source account to pay for creating destination token account
        let token_account_rent = rent.minimum_balance(spl_token::state::Account::LEN);
        context
            .banks_client
            .process_transaction(Transaction::new_signed_with_payer(
                &[system_instruction::transfer(
                    &context.payer.pubkey(),
                    source_account,
                    token_account_rent,
                )],
                Some(&context.payer.pubkey()),
                &[&context.payer],
                context.recent_blockhash,
            ))
            .await
            .unwrap();
    }

    SPLTestContext {
        mint,
        mint_authority,
        source_token_address,
        destination_token_address,
    }
}

pub async fn get_token_balance(context: &mut BalanceAccountTestContext, account: &Pubkey) -> u64 {
    spl_token::state::Account::unpack_from_slice(
        context
            .banks_client
            .get_account(*account)
            .await
            .unwrap()
            .unwrap()
            .data
            .as_slice(),
    )
    .unwrap()
    .amount
}

pub async fn get_wallet(banks_client: &mut BanksClient, account: &Pubkey) -> Wallet {
    Wallet::unpack_from_slice(
        banks_client
            .get_account(*account)
            .await
            .unwrap()
            .unwrap()
            .data(),
    )
    .unwrap()
}

pub fn assert_multisig_op_timestamps(
    multisig_op: &MultisigOp,
    initialized_at: SystemTime,
    approval_timeout: Duration,
) {
    let initialized_at_timestamp =
        initialized_at.duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;

    assert!(multisig_op.started_at - initialized_at_timestamp <= 2);
    assert!(
        multisig_op.expires_at - initialized_at_timestamp - approval_timeout.as_secs() as i64 <= 2
    );
}

pub fn assert_initialized_multisig_op(
    multisig_op: &MultisigOp,
    initialized_at: SystemTime,
    expected_approval_timeout: Duration,
    expected_dispositions_required: u8,
    expected_dispositions: &Vec<ApprovalDispositionRecord>,
    expected_op_disposition: OperationDisposition,
    expected_params: &MultisigOpParams,
) {
    assert!(multisig_op.is_initialized);
    assert_multisig_op_timestamps(&multisig_op, initialized_at, expected_approval_timeout);
    assert_eq!(
        multisig_op.dispositions_required,
        expected_dispositions_required
    );
    assert_eq!(
        multisig_op.disposition_records.to_set(),
        expected_dispositions.to_set()
    );
    assert_eq!(multisig_op.operation_disposition, expected_op_disposition);
    assert_eq!(multisig_op.params_hash, expected_params.hash());
}

pub async fn verify_multisig_op_init_fails(
    banks_client: &mut BanksClient,
    recent_blockhash: Hash,
    payer: &Keypair,
    assistant_account: &Keypair,
    multisig_op_account: &Keypair,
    init_instruction: Instruction,
    expected_error: InstructionError,
) {
    let transaction = Transaction::new_signed_with_payer(
        &[
            system_instruction::create_account(
                &payer.pubkey(),
                &multisig_op_account.pubkey(),
                banks_client
                    .get_rent()
                    .await
                    .unwrap()
                    .minimum_balance(MultisigOp::LEN),
                MultisigOp::LEN as u64,
                &init_instruction.program_id,
            ),
            init_instruction,
        ],
        Some(&payer.pubkey()),
        &[&payer, multisig_op_account, &assistant_account],
        recent_blockhash,
    );

    assert_eq!(
        banks_client
            .process_transaction(transaction)
            .await
            .unwrap_err()
            .unwrap(),
        TransactionError::InstructionError(1, expected_error),
    );
}

pub async fn process_wrap(
    context: &mut BalanceAccountTestContext,
    multisig_account_rent: u64,
    balance_account: Pubkey,
    amount: u64,
    token_account_rent: u64,
    wrapped_sol_account: Pubkey,
) -> transport::Result<()> {
    let multisig_op_account = Keypair::new();

    let init_result = context
        .banks_client
        .process_transaction(Transaction::new_signed_with_payer(
            &[
                system_instruction::create_account(
                    &context.payer.pubkey(),
                    &multisig_op_account.pubkey(),
                    multisig_account_rent,
                    MultisigOp::LEN as u64,
                    &context.program_id,
                ),
                instructions::init_wrap_unwrap(
                    &context.program_id,
                    &context.wallet_account.pubkey(),
                    &multisig_op_account.pubkey(),
                    &context.assistant_account.pubkey(),
                    &balance_account,
                    &context.balance_account_guid_hash,
                    amount,
                    WrapDirection::WRAP,
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
        .await;

    if let Err(_) = init_result {
        return init_result;
    }

    assert_eq!(
        context
            .banks_client
            .get_balance(wrapped_sol_account)
            .await
            .unwrap(),
        token_account_rent
    );

    assert_eq!(
        get_token_balance(context.borrow_mut(), &wrapped_sol_account).await,
        0
    );

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

    context
        .banks_client
        .process_transaction(Transaction::new_signed_with_payer(
            &[instructions::finalize_wrap_unwrap(
                &context.program_id,
                &multisig_op_account.pubkey(),
                &context.wallet_account.pubkey(),
                &balance_account,
                &context.payer.pubkey(),
                &context.balance_account_guid_hash,
                amount,
                WrapDirection::WRAP,
            )],
            Some(&context.payer.pubkey()),
            &[&context.payer],
            context.recent_blockhash,
        ))
        .await
}

pub async fn process_unwrapping(
    context: &mut BalanceAccountTestContext,
    multisig_account_rent: u64,
    balance_account: Pubkey,
    unwrap_amount: u64,
) -> transport::Result<()> {
    let unwrap_multisig_op_account = Keypair::new();

    context
        .banks_client
        .process_transaction(Transaction::new_signed_with_payer(
            &[
                system_instruction::create_account(
                    &context.payer.pubkey(),
                    &unwrap_multisig_op_account.pubkey(),
                    multisig_account_rent,
                    MultisigOp::LEN as u64,
                    &context.program_id,
                ),
                instructions::init_wrap_unwrap(
                    &context.program_id,
                    &context.wallet_account.pubkey(),
                    &unwrap_multisig_op_account.pubkey(),
                    &context.assistant_account.pubkey(),
                    &balance_account,
                    &context.balance_account_guid_hash,
                    unwrap_amount,
                    WrapDirection::UNWRAP,
                ),
            ],
            Some(&context.payer.pubkey()),
            &[
                &context.payer,
                &unwrap_multisig_op_account,
                &context.assistant_account,
            ],
            context.recent_blockhash,
        ))
        .await
        .unwrap();

    approve_or_deny_n_of_n_multisig_op(
        context.banks_client.borrow_mut(),
        &context.program_id,
        &unwrap_multisig_op_account.pubkey(),
        vec![&context.approvers[0], &context.approvers[1]],
        &context.payer,
        context.recent_blockhash,
        ApprovalDisposition::APPROVE,
        OperationDisposition::APPROVED,
    )
    .await;

    context
        .banks_client
        .process_transaction(Transaction::new_signed_with_payer(
            &[instructions::finalize_wrap_unwrap(
                &context.program_id,
                &unwrap_multisig_op_account.pubkey(),
                &context.wallet_account.pubkey(),
                &balance_account,
                &context.payer.pubkey(),
                &context.balance_account_guid_hash,
                unwrap_amount,
                WrapDirection::UNWRAP,
            )],
            Some(&context.payer.pubkey()),
            &[&context.payer],
            context.recent_blockhash,
        ))
        .await
}
