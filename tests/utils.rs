use arrayref::array_ref;
use itertools::Itertools;
use sha2::{Digest, Sha256};
use solana_program::instruction::{Instruction, InstructionError};
use solana_program::system_program;
use solana_program_test::{processor, ProgramTest};
use solana_sdk::account::ReadableAccount;
use solana_sdk::transaction::TransactionError;
use solana_sdk::transport;
use std::borrow::BorrowMut;
use std::collections::HashSet;
use std::time::Duration;
use strike_wallet::instruction::{
    finalize_wallet_creation, finalize_wrap_unwrap, init_transfer, init_wallet_creation,
    init_wrap_unwrap, program_init_config_update, set_approval_disposition, ProgramConfigUpdate,
    WalletConfigUpdate,
};
use strike_wallet::model::multisig_op::{
    ApprovalDisposition, ApprovalDispositionRecord, MultisigOp, MultisigOpParams,
    OperationDisposition, WrapDirection,
};
use strike_wallet::model::program_config::{AddressBook, Approvers, Signers};
use strike_wallet::model::signer::Signer;
use strike_wallet::model::wallet_config::AddressBookEntry;
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
    strike_wallet::{
        instruction::program_init, model::program_config::ProgramConfig, processor::Processor,
    },
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

pub async fn init_program(
    banks_client: &mut BanksClient,
    payer: &Keypair,
    recent_blockhash: Hash,
    program_owner: &Keypair,
    program_config_account: &Keypair,
    assistant_account: &Keypair,
    approvals_required_for_config: Option<u8>,
    signers: Option<Vec<(SlotId<Signer>, Signer)>>,
    config_approvers: Option<Vec<(SlotId<Signer>, Signer)>>,
    approval_timeout_for_config: Option<Duration>,
    address_book: Option<Vec<(SlotId<AddressBookEntry>, AddressBookEntry)>>,
) -> Result<(), TransportError> {
    let rent = banks_client.get_rent().await.unwrap();
    let program_rent = rent.minimum_balance(ProgramConfig::LEN);

    let transaction = Transaction::new_signed_with_payer(
        &[
            system_instruction::create_account(
                &payer.pubkey(),
                &program_config_account.pubkey(),
                program_rent,
                ProgramConfig::LEN as u64,
                &program_owner.pubkey(),
            ),
            program_init(
                &program_owner.pubkey(),
                &program_config_account.pubkey(),
                &assistant_account.pubkey(),
                signers.unwrap_or(Vec::new()),
                config_approvers.unwrap_or(Vec::new()),
                approvals_required_for_config.unwrap_or(0),
                approval_timeout_for_config.unwrap_or(Duration::from_secs(0)),
                address_book.unwrap_or(Vec::new()),
            ),
        ],
        Some(&payer.pubkey()),
        &[payer, program_config_account, assistant_account],
        recent_blockhash,
    );
    banks_client.process_transaction(transaction).await?;
    Ok(())
}

pub struct ProgramConfigUpdateContext {
    pub payer: Keypair,
    pub program_owner: Keypair,
    pub banks_client: BanksClient,
    pub program_config_account: Keypair,
    pub multisig_op_account: Keypair,
    pub approvers: Vec<Keypair>,
    pub recent_blockhash: Hash,
    pub expected_config_update: ProgramConfigUpdate,
    pub params_hash: Hash,
    pub expected_config_after_update: ProgramConfig,
}

pub async fn setup_program_config_update_test() -> ProgramConfigUpdateContext {
    let program_owner = Keypair::new();
    let mut pt = ProgramTest::new(
        "strike_wallet",
        program_owner.pubkey(),
        processor!(Processor::process),
    );
    pt.set_bpf_compute_max_units(30_000);
    let (mut banks_client, payer, recent_blockhash) = pt.start().await;
    let program_config_account = Keypair::new();
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
        name_hash: [0; 32],
    };
    let new_address_book_entry = AddressBookEntry {
        address: Pubkey::new_unique(),
        name_hash: [0; 32],
    };

    // first initialize the program config
    init_program(
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

    let program_config =
        get_program_config(&mut banks_client, &program_config_account.pubkey()).await;

    // now initialize a config update
    let rent = banks_client.get_rent().await.unwrap();
    let multisig_account_rent = rent.minimum_balance(MultisigOp::LEN);
    let init_transaction = Transaction::new_signed_with_payer(
        &[
            system_instruction::create_account(
                &payer.pubkey(),
                &multisig_op_account.pubkey(),
                multisig_account_rent,
                MultisigOp::LEN as u64,
                &program_owner.pubkey(),
            ),
            program_init_config_update(
                &program_owner.pubkey(),
                &program_config_account.pubkey(),
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
        Some(&payer.pubkey()),
        &[&payer, &multisig_op_account, &assistant_account],
        recent_blockhash,
    );
    banks_client
        .process_transaction(init_transaction)
        .await
        .unwrap();

    let expected_config_update = ProgramConfigUpdate {
        approvals_required_for_config: 2,
        approval_timeout_for_config: Duration::from_secs(7200),
        add_signers: vec![(SlotId::new(2), signers[2])],
        remove_signers: vec![(SlotId::new(0), signers[0])],
        add_config_approvers: vec![(SlotId::new(2), signers[2])],
        remove_config_approvers: vec![(SlotId::new(0), signers[0])],
        add_address_book_entries: vec![(SlotId::new(0), new_address_book_entry)],
        remove_address_book_entries: vec![(SlotId::new(0), address_book_entry)],
    };

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

    ProgramConfigUpdateContext {
        payer,
        program_owner,
        banks_client,
        program_config_account,
        multisig_op_account,
        approvers,
        recent_blockhash,
        expected_config_update,
        params_hash: multisig_op.params_hash,
        expected_config_after_update: ProgramConfig {
            is_initialized: true,
            signers: Signers::from_vec(vec![
                (SlotId::new(1), signers[1]),
                (SlotId::new(2), signers[2]),
            ]),
            assistant: program_config.assistant,
            address_book: AddressBook::from_vec(vec![(SlotId::new(0), new_address_book_entry)]),
            approvals_required_for_config: 2,
            approval_timeout_for_config: Duration::from_secs(7200),
            config_approvers: Approvers::from_enabled_vec(vec![SlotId::new(1), SlotId::new(2)]),
            wallets: program_config.wallets,
        },
    }
}

pub async fn approve_or_deny_n_of_n_multisig_op(
    banks_client: &mut BanksClient,
    program_owner: &Pubkey,
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
                program_owner,
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
                disposition: disposition,
            })
            .collect_vec()
            .to_set()
    );
    assert_eq!(
        multisig_op.operation_disposition,
        expected_operation_disposition
    )
}

pub async fn approve_or_deny_1_of_2_multisig_op(
    banks_client: &mut BanksClient,
    program_owner: &Pubkey,
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
            program_owner,
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
                disposition: disposition,
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

pub struct WalletTestContext {
    pub payer: Keypair,
    pub program_owner: Keypair,
    pub banks_client: BanksClient,
    pub program_config_account: Keypair,
    pub multisig_op_account: Keypair,
    pub assistant_account: Keypair,
    pub approvers: Vec<Keypair>,
    pub recent_blockhash: Hash,
    pub expected_config_update: WalletConfigUpdate,
    pub wallet_name_hash: [u8; 32],
    pub wallet_guid_hash: [u8; 32],
    pub destination_name_hash: [u8; 32],
    pub allowed_destination: AddressBookEntry,
    pub destination: Keypair,
    pub params_hash: Hash,
}

pub async fn setup_wallet_tests(bpf_compute_max_units: Option<u64>) -> WalletTestContext {
    let program_owner = Keypair::new();
    let mut pt = ProgramTest::new(
        "strike_wallet",
        program_owner.pubkey(),
        processor!(Processor::process),
    );
    pt.set_bpf_compute_max_units(bpf_compute_max_units.unwrap_or(25_000));
    let (mut banks_client, payer, recent_blockhash) = pt.start().await;
    let program_config_account = Keypair::new();
    let multisig_op_account = Keypair::new();
    let assistant_account = Keypair::new();

    let approvers = vec![Keypair::new(), Keypair::new(), Keypair::new()];

    let destination = Keypair::new();
    let addr_book_entry = AddressBookEntry {
        address: destination.pubkey(),
        name_hash: hash_of(b"Destination 1 Name"),
    };
    let addr_book_entry2 = AddressBookEntry {
        address: Keypair::new().pubkey(),
        name_hash: hash_of(b"Destination 2 Name"),
    };

    // first initialize the program config
    init_program(
        &mut banks_client,
        &payer,
        recent_blockhash,
        &program_owner,
        &program_config_account,
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

    // now initialize wallet creation
    let rent = banks_client.get_rent().await.unwrap();
    let multisig_account_rent = rent.minimum_balance(MultisigOp::LEN);
    let wallet_guid = Uuid::new_v4();
    let account_name_hash = hash_of(b"Account Name");
    let wallet_guid_hash = hash_of(wallet_guid.as_bytes());

    let init_transaction = Transaction::new_signed_with_payer(
        &[
            system_instruction::create_account(
                &payer.pubkey(),
                &multisig_op_account.pubkey(),
                multisig_account_rent,
                MultisigOp::LEN as u64,
                &program_owner.pubkey(),
            ),
            init_wallet_creation(
                &program_owner.pubkey(),
                &program_config_account.pubkey(),
                &multisig_op_account.pubkey(),
                &assistant_account.pubkey(),
                wallet_guid_hash,
                account_name_hash,
                2,
                Duration::from_secs(1800),
                vec![
                    (SlotId::new(0), approvers[0].pubkey_as_signer()),
                    (SlotId::new(1), approvers[1].pubkey_as_signer()),
                ],
                vec![(SlotId::new(0), addr_book_entry)],
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

    let expected_config_update = WalletConfigUpdate {
        name_hash: *array_ref!(account_name_hash, 0, 32),
        approvals_required_for_transfer: 2,
        approval_timeout_for_transfer: Duration::from_secs(1800),
        add_transfer_approvers: vec![
            (SlotId::new(0), approvers[0].pubkey_as_signer()),
            (SlotId::new(1), approvers[1].pubkey_as_signer()),
        ],
        remove_transfer_approvers: vec![],
        add_allowed_destinations: vec![(SlotId::new(0), addr_book_entry)],
        remove_allowed_destinations: vec![],
    };

    assert_eq!(
        multisig_op.params_hash,
        MultisigOpParams::CreateWallet {
            program_config_address: program_config_account.pubkey(),
            wallet_guid_hash,
            config_update: expected_config_update.clone(),
        }
        .hash()
    );

    WalletTestContext {
        payer,
        program_owner,
        banks_client,
        program_config_account,
        multisig_op_account,
        assistant_account,
        approvers,
        recent_blockhash,
        expected_config_update,
        wallet_name_hash: account_name_hash,
        wallet_guid_hash,
        destination_name_hash: addr_book_entry.name_hash,
        allowed_destination: addr_book_entry,
        destination,
        params_hash: multisig_op.params_hash,
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

pub async fn setup_init_wallet_failure_tests(
    bpf_compute_max_units: Option<u64>,
    approvals_required_for_transfer: u8,
    approval_timeout_for_transfer: Duration,
    transfer_approvers: Vec<Pubkey>,
) -> TransactionError {
    let program_owner = Keypair::new();
    let mut pt = ProgramTest::new(
        "strike_wallet",
        program_owner.pubkey(),
        processor!(Processor::process),
    );
    pt.set_bpf_compute_max_units(bpf_compute_max_units.unwrap_or(25_000));
    let (mut banks_client, payer, recent_blockhash) = pt.start().await;
    let program_config_account = Keypair::new();
    let multisig_op_account = Keypair::new();
    let assistant_account = Keypair::new();

    let approvers = vec![Keypair::new(), Keypair::new(), Keypair::new()];

    // first initialize the program config
    init_program(
        &mut banks_client,
        &payer,
        recent_blockhash,
        &program_owner,
        &program_config_account,
        &assistant_account,
        Some(1),
        Some(vec![
            (SlotId::new(0), approvers[0].pubkey_as_signer()),
            (SlotId::new(1), approvers[1].pubkey_as_signer()),
        ]),
        Some(vec![
            (SlotId::new(0), approvers[0].pubkey_as_signer()),
            (SlotId::new(1), approvers[1].pubkey_as_signer()),
        ]),
        Some(Duration::from_secs(3600)),
        Some(vec![]),
    )
    .await
    .unwrap();

    // now initialize a wallet creation
    let rent = banks_client.get_rent().await.unwrap();
    let multisig_account_rent = rent.minimum_balance(MultisigOp::LEN);
    let wallet_guid = Uuid::new_v4();
    let account_name_hash = hash_of(b"Account Name");
    let wallet_guid_hash = hash_of(wallet_guid.as_bytes());

    let init_transaction = Transaction::new_signed_with_payer(
        &[
            system_instruction::create_account(
                &payer.pubkey(),
                &multisig_op_account.pubkey(),
                multisig_account_rent,
                MultisigOp::LEN as u64,
                &program_owner.pubkey(),
            ),
            init_wallet_creation(
                &program_owner.pubkey(),
                &program_config_account.pubkey(),
                &multisig_op_account.pubkey(),
                &assistant_account.pubkey(),
                wallet_guid_hash,
                account_name_hash,
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

pub async fn finalize_wallet(context: &mut WalletTestContext) {
    let finalize_transaction = Transaction::new_signed_with_payer(
        &[finalize_wallet_creation(
            &context.program_owner.pubkey(),
            &context.program_config_account.pubkey(),
            &context.multisig_op_account.pubkey(),
            &context.payer.pubkey(),
            context.wallet_guid_hash,
            context.expected_config_update.clone(),
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

pub async fn setup_wallet_tests_and_finalize(
    bpf_compute_max_units: Option<u64>,
) -> (WalletTestContext, Pubkey) {
    let mut context = setup_wallet_tests(bpf_compute_max_units).await;

    approve_or_deny_1_of_2_multisig_op(
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

    finalize_wallet(context.borrow_mut()).await;
    let (source_account, _) = Pubkey::find_program_address(
        &[&context.wallet_guid_hash],
        &context.program_owner.pubkey(),
    );

    (context, source_account)
}

pub async fn setup_transfer_test(
    context: &mut WalletTestContext,
    balance_account: &Pubkey,
    token_mint: Option<&Pubkey>,
    amount: Option<u64>,
) -> (Keypair, transport::Result<()>) {
    let rent = context.banks_client.get_rent().await.unwrap();
    let multisig_account_rent = rent.minimum_balance(MultisigOp::LEN);
    let multisig_op_account = Keypair::new();

    let result = context
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
                init_transfer(
                    &context.program_owner.pubkey(),
                    &context.program_config_account.pubkey(),
                    &multisig_op_account.pubkey(),
                    &context.assistant_account.pubkey(),
                    &balance_account,
                    &context.destination.pubkey(),
                    context.wallet_guid_hash,
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

    (multisig_op_account, result)
}

pub struct SPLTestContext {
    pub mint: Keypair,
    pub mint_authority: Keypair,
    pub source_token_address: Pubkey,
    pub destination_token_address: Pubkey,
}

pub async fn setup_spl_transfer_test(
    context: &mut WalletTestContext,
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

pub async fn get_token_balance(context: &mut WalletTestContext, account: &Pubkey) -> u64 {
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

pub async fn get_program_config(banks_client: &mut BanksClient, account: &Pubkey) -> ProgramConfig {
    ProgramConfig::unpack_from_slice(
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
    start: i64,
    approval_timeout: Duration,
) {
    assert!(multisig_op.started_at - start <= 2);
    assert!(multisig_op.expires_at - start - approval_timeout.as_secs() as i64 <= 2);
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
    context: &mut WalletTestContext,
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
                    &context.program_owner.pubkey(),
                ),
                init_wrap_unwrap(
                    &context.program_owner.pubkey(),
                    &context.program_config_account.pubkey(),
                    &multisig_op_account.pubkey(),
                    &context.assistant_account.pubkey(),
                    &balance_account,
                    context.wallet_guid_hash,
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
        &context.program_owner.pubkey(),
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
            &[finalize_wrap_unwrap(
                &context.program_owner.pubkey(),
                &multisig_op_account.pubkey(),
                &context.program_config_account.pubkey(),
                &balance_account,
                &context.payer.pubkey(),
                context.wallet_guid_hash,
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
    context: &mut WalletTestContext,
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
                    &context.program_owner.pubkey(),
                ),
                init_wrap_unwrap(
                    &context.program_owner.pubkey(),
                    &context.program_config_account.pubkey(),
                    &unwrap_multisig_op_account.pubkey(),
                    &context.assistant_account.pubkey(),
                    &balance_account,
                    context.wallet_guid_hash,
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
        &context.program_owner.pubkey(),
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
            &[finalize_wrap_unwrap(
                &context.program_owner.pubkey(),
                &unwrap_multisig_op_account.pubkey(),
                &context.program_config_account.pubkey(),
                &balance_account,
                &context.payer.pubkey(),
                context.wallet_guid_hash,
                unwrap_amount,
                WrapDirection::UNWRAP,
            )],
            Some(&context.payer.pubkey()),
            &[&context.payer],
            context.recent_blockhash,
        ))
        .await
}
