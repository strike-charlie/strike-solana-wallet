use arrayref::array_ref;
use sha2::{Digest, Sha256};
use solana_program::system_program;
use solana_program_test::{processor, ProgramTest};
use solana_sdk::account::ReadableAccount;
use solana_sdk::transaction::TransactionError;
use solana_sdk::transport;
use std::borrow::BorrowMut;
use std::time::Duration;
use strike_wallet::instruction::{
    finalize_wallet_creation, init_transfer, init_wallet_config_update, init_wallet_creation,
    program_init_config_update, set_approval_disposition, ProgramConfigUpdate, WalletConfigUpdate,
};
use strike_wallet::model::multisig_op::{
    ApprovalDisposition, ApprovalDispositionRecord, MultisigOp, MultisigOpParams,
    OperationDisposition
};
use strike_wallet::model::wallet_config::{AllowedDestination, WalletConfig};
use uuid::Uuid;
use {
    solana_program::{program_pack::Pack, pubkey::Pubkey},
    solana_program_test::BanksClient,
    solana_sdk::{
        hash::Hash,
        signature::{Keypair, Signer},
        system_instruction,
        transaction::Transaction,
        transport::TransportError,
    },
    strike_wallet::{
        instruction::program_init, model::program_config::ProgramConfig, processor::Processor,
    },
};

pub async fn init_program(
    banks_client: &mut BanksClient,
    payer: &Keypair,
    recent_blockhash: Hash,
    program_owner: &Keypair,
    program_config_account: &Keypair,
    assistant_account: &Keypair,
    approvals_required_for_config: Option<u8>,
    config_approvers: Option<Vec<Pubkey>>,
    approval_timeout_for_config: Option<Duration>,
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
                config_approvers.unwrap_or(Vec::new()),
                approvals_required_for_config.unwrap_or(0),
                approval_timeout_for_config.unwrap_or(Duration::from_secs(0)),
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
    pub params_hash: Hash
}

pub async fn setup_program_config_update_test() -> ProgramConfigUpdateContext {
    let program_owner = Keypair::new();
    let mut pt = ProgramTest::new(
        "strike_wallet",
        program_owner.pubkey(),
        processor!(Processor::process),
    );
    pt.set_bpf_compute_max_units(10_000);
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
        Some(vec![approvers[0].pubkey(), approvers[1].pubkey()]),
        Some(Duration::from_secs(3600))
    )
    .await
    .unwrap();

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
                vec![approvers[2].pubkey()],
                vec![approvers[0].pubkey()],
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
        add_approvers: vec![approvers[2].pubkey()],
        remove_approvers: vec![approvers[0].pubkey()],
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
        params_hash: multisig_op.params_hash
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
    expected_operation_disposition: OperationDisposition
) {

    let params_hash = get_operation_hash(
        banks_client.borrow_mut(),
        *multisig_op_account).await;

    // approve the config change
    for approver in approvers.iter() {
        let approve_transaction = Transaction::new_signed_with_payer(
            &[set_approval_disposition(
                program_owner,
                multisig_op_account,
                &approver.pubkey(),
                disposition,
                params_hash
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
        multisig_op.disposition_records,
        approvers
            .iter()
            .map(|approver| ApprovalDispositionRecord {
                approver: approver.pubkey(),
                disposition: disposition,
            })
            .collect::<Vec<ApprovalDispositionRecord>>()
    );
    assert_eq!(multisig_op.operation_disposition, expected_operation_disposition)
}

pub async fn approve_or_deny_1_of_2_multisig_op(
    banks_client: &mut BanksClient,
    program_owner: &Pubkey,
    multisig_op_account: &Pubkey,
    approver: &Keypair,
    payer: &Keypair,
    other_approver: &Pubkey,
    recent_blockhash: Hash,
    disposition: ApprovalDisposition
) {
    let params_hash = get_operation_hash(
        banks_client.borrow_mut(),
        *multisig_op_account).await;

    // approve the config change
    let approve_transaction = Transaction::new_signed_with_payer(
        &[set_approval_disposition(
            program_owner,
            multisig_op_account,
            &approver.pubkey(),
            disposition,
            params_hash
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
        multisig_op.disposition_records,
        vec![
            ApprovalDispositionRecord {
                approver: approver.pubkey(),
                disposition: disposition,
            },
            ApprovalDispositionRecord {
                approver: *other_approver,
                disposition: ApprovalDisposition::NONE,
            },
        ]
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
    pub allowed_destination: AllowedDestination,
    pub destination: Keypair,
    pub params_hash: Hash
}

pub async fn setup_wallet_tests(bpf_compute_max_units: Option<u64>) -> WalletTestContext {
    let program_owner = Keypair::new();
    let mut pt = ProgramTest::new(
        "strike_wallet",
        program_owner.pubkey(),
        processor!(Processor::process),
    );
    pt.set_bpf_compute_max_units(bpf_compute_max_units.unwrap_or(20_000));
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
        Some(vec![approvers[0].pubkey(), approvers[1].pubkey()]),
        Some(Duration::from_secs(3600))
    )
    .await
    .unwrap();

    // now initialize a wallet creation
    let rent = banks_client.get_rent().await.unwrap();
    let multisig_account_rent = rent.minimum_balance(MultisigOp::LEN);
    let wallet_guid = Uuid::new_v4();
    let account_name_hash = hash_of(b"Account Name");
    let destination = Keypair::new();
    let destination_name_hash = hash_of(b"Destination Name");
    let wallet_guid_hash = hash_of(wallet_guid.as_bytes());
    let allowed_destination = AllowedDestination {
        address: destination.pubkey(),
        name_hash: destination_name_hash,
    };

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
                vec![approvers[1].pubkey(), approvers[2].pubkey()],
                vec![allowed_destination],
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
        multisig_op.disposition_records,
        vec![
            ApprovalDispositionRecord {
                approver: approvers[0].pubkey(),
                disposition: ApprovalDisposition::NONE,
            },
            ApprovalDispositionRecord {
                approver: approvers[1].pubkey(),
                disposition: ApprovalDisposition::NONE,
            },
        ]
    );
    assert_eq!(multisig_op.dispositions_required, 1);

    let expected_config_update = WalletConfigUpdate {
        name_hash: *array_ref!(account_name_hash, 0, 32),
        approvals_required_for_transfer: 2,
        approval_timeout_for_transfer: Duration::from_secs(1800),
        add_approvers: vec![approvers[1].pubkey(), approvers[2].pubkey()],
        remove_approvers: vec![],
        add_allowed_destinations: vec![allowed_destination],
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
        destination_name_hash,
        allowed_destination,
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
    ).unwrap();

    multisig_op.params_hash
}

pub async fn setup_init_wallet_failure_tests(bpf_compute_max_units: Option<u64>,
                                             approvals_required_for_transfer: u8,
                                             approval_timeout_for_transfer: Duration,
                                             transfer_approvers: Vec<Pubkey>) -> TransactionError {
    let program_owner = Keypair::new();
    let mut pt = ProgramTest::new(
        "strike_wallet",
        program_owner.pubkey(),
        processor!(Processor::process),
    );
    pt.set_bpf_compute_max_units(bpf_compute_max_units.unwrap_or(20_000));
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
        Some(vec![approvers[0].pubkey(), approvers[1].pubkey()]),
        Some(Duration::from_secs(3600))
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
                transfer_approvers,
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

pub async fn finalize_wallet(context: &mut WalletTestContext) -> Keypair {
    let wallet_account = Keypair::new();
    let rent = context.banks_client.get_rent().await.unwrap();
    let wallet_account_rent = rent.minimum_balance(WalletConfig::LEN);
    let finalize_transaction = Transaction::new_signed_with_payer(
        &[
            system_instruction::create_account(
                &context.payer.pubkey(),
                &wallet_account.pubkey(),
                wallet_account_rent,
                WalletConfig::LEN as u64,
                &context.program_owner.pubkey(),
            ),
            finalize_wallet_creation(
                &context.program_owner.pubkey(),
                &context.program_config_account.pubkey(),
                &wallet_account.pubkey(),
                &context.multisig_op_account.pubkey(),
                &context.payer.pubkey(),
                context.wallet_guid_hash,
                context.expected_config_update.clone(),
            ),
        ],
        Some(&context.payer.pubkey()),
        &[&context.payer, &wallet_account],
        context.recent_blockhash,
    );
    context
        .banks_client
        .process_transaction(finalize_transaction)
        .await
        .unwrap();
    wallet_account
}

pub async fn setup_wallet_tests_and_finalize(
    bpf_compute_max_units: Option<u64>,
) -> (WalletTestContext, Keypair, Pubkey) {
    let mut context = setup_wallet_tests(bpf_compute_max_units).await;

    approve_or_deny_1_of_2_multisig_op(
        context.banks_client.borrow_mut(),
        &context.program_owner.pubkey(),
        &context.multisig_op_account.pubkey(),
        &context.approvers[0],
        &context.payer,
        &context.approvers[1].pubkey(),
        context.recent_blockhash,
        ApprovalDisposition::APPROVE
    )
    .await;

    let account_data = finalize_wallet(context.borrow_mut()).await;
    let (source_account, _) = Pubkey::find_program_address(
        &[&account_data.pubkey().to_bytes()],
        &context.program_owner.pubkey(),
    );

    (context, account_data, source_account)
}

pub async fn add_n_destinations(
    context: &mut WalletTestContext,
    wallet_account: &Pubkey,
    n: usize,
) -> (Transaction, Keypair, WalletConfigUpdate) {
    let rent = context.banks_client.get_rent().await.unwrap();
    let multisig_account_rent = rent.minimum_balance(MultisigOp::LEN);
    let multisig_op_account = Keypair::new();

    let new_destinations = (1..n)
        .map(|_| AllowedDestination {
            address: Pubkey::new_unique(),
            name_hash: [0; 32],
        })
        .collect::<Vec<AllowedDestination>>();
    let expected_config = WalletConfigUpdate {
        name_hash: context.wallet_name_hash,
        approvals_required_for_transfer: 2,
        approval_timeout_for_transfer: Duration::from_secs(3600),
        add_approvers: vec![],
        remove_approvers: vec![],
        add_allowed_destinations: new_destinations.clone(),
        remove_allowed_destinations: vec![],
    };
    (
        Transaction::new_signed_with_payer(
            &[
                system_instruction::create_account(
                    &context.payer.pubkey(),
                    &multisig_op_account.pubkey(),
                    multisig_account_rent,
                    MultisigOp::LEN as u64,
                    &context.program_owner.pubkey(),
                ),
                init_wallet_config_update(
                    &context.program_owner.pubkey(),
                    &context.program_config_account.pubkey(),
                    &multisig_op_account.pubkey(),
                    &context.assistant_account.pubkey(),
                    &wallet_account,
                    context.wallet_guid_hash,
                    context.wallet_name_hash,
                    2,
                    Duration::from_secs(3600),
                    vec![],
                    vec![],
                    new_destinations.clone(),
                    vec![],
                ),
            ],
            Some(&context.payer.pubkey()),
            &[
                &context.payer,
                &multisig_op_account,
                &context.assistant_account,
            ],
            context.recent_blockhash,
        ),
        multisig_op_account,
        expected_config,
    )
}

pub async fn setup_transfer_test(
    context: &mut WalletTestContext,
    wallet_account: &Keypair,
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
                    &wallet_account.pubkey(),
                    &balance_account,
                    &context.destination.pubkey(),
                    amount.unwrap_or(123),
                    context.destination_name_hash,
                    token_mint.unwrap_or(&system_program::id()),
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
                spl_associated_token_account::create_associated_token_account(
                    &context.payer.pubkey(),
                    &context.destination.pubkey(),
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
            ],
            Some(&context.payer.pubkey()),
            &[&context.payer, &mint, &mint_authority, &context.destination],
            context.recent_blockhash,
        ))
        .await
        .unwrap();

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
