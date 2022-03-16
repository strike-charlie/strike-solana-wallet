use solana_program::hash::Hash;

use solana_program::instruction::{AccountMeta, Instruction};
use solana_program::pubkey::Pubkey;
use solana_program::{system_program, sysvar};
use std::borrow::Borrow;
use std::time::Duration;
use strike_wallet::instruction::{BalanceAccountCreation, BalanceAccountPolicyUpdate};
use strike_wallet::model::balance_account::BalanceAccount;
use strike_wallet::{
    instruction::{
        AddressBookUpdate, BalanceAccountWhitelistUpdate, DAppBookUpdate, InitialWalletConfig,
        ProgramInstruction, WalletConfigPolicyUpdate,
    },
    model::{
        address_book::{AddressBookEntry, AddressBookEntryNameHash, DAppBookEntry},
        balance_account::{BalanceAccountGuidHash, BalanceAccountNameHash},
        multisig_op::{ApprovalDisposition, BooleanSetting, SlotUpdateType, WrapDirection},
        signer::Signer,
    },
    utils,
    utils::SlotId,
};

pub fn init_wallet(
    program_id: &Pubkey,
    wallet_account: &Pubkey,
    assistant_account: &Pubkey,
    initial_config: InitialWalletConfig,
) -> Instruction {
    let accounts = vec![
        AccountMeta::new(*wallet_account, false),
        AccountMeta::new_readonly(*assistant_account, true),
    ];

    Instruction {
        program_id: *program_id,
        accounts,
        data: ProgramInstruction::InitWallet { initial_config }
            .borrow()
            .pack(),
    }
}

fn init_multisig_op(
    program_id: &Pubkey,
    wallet_account: &Pubkey,
    multisig_op_account: &Pubkey,
    initiator_account: &Pubkey,
    program_instruction: ProgramInstruction,
) -> Instruction {
    let mut accounts = vec![AccountMeta::new(*multisig_op_account, false)];
    accounts.push(AccountMeta::new_readonly(*wallet_account, false));
    accounts.push(AccountMeta::new_readonly(*initiator_account, true));
    accounts.push(AccountMeta::new_readonly(sysvar::clock::id(), false));

    Instruction {
        program_id: *program_id,
        accounts,
        data: program_instruction.borrow().pack(),
    }
}

pub fn set_approval_disposition(
    program_id: &Pubkey,
    multisig_op_account: &Pubkey,
    approver: &Pubkey,
    disposition: ApprovalDisposition,
    params_hash: Hash,
) -> Instruction {
    let data = ProgramInstruction::SetApprovalDisposition {
        disposition,
        params_hash,
    }
    .borrow()
    .pack();

    let accounts = vec![
        AccountMeta::new(*multisig_op_account, false),
        AccountMeta::new_readonly(*approver, true),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
    ];

    Instruction {
        program_id: *program_id,
        accounts,
        data,
    }
}

pub fn init_balance_account_creation(
    program_id: &Pubkey,
    wallet_account: &Pubkey,
    multisig_op_account: &Pubkey,
    initiator_account: &Pubkey,
    slot_id: SlotId<BalanceAccount>,
    account_guid_hash: BalanceAccountGuidHash,
    name_hash: BalanceAccountNameHash,
    approvals_required_for_transfer: u8,
    approval_timeout_for_transfer: Duration,
    approvers: Vec<(SlotId<Signer>, Signer)>,
    whitelist_enabled: BooleanSetting,
    dapps_enabled: BooleanSetting,
    address_book_slot_id: SlotId<AddressBookEntry>,
) -> Instruction {
    init_multisig_op(
        program_id,
        wallet_account,
        multisig_op_account,
        initiator_account,
        ProgramInstruction::InitBalanceAccountCreation {
            account_guid_hash,
            creation_params: BalanceAccountCreation {
                slot_id,
                name_hash,
                approvals_required_for_transfer,
                approval_timeout_for_transfer,
                transfer_approvers: approvers.clone(),
                whitelist_enabled,
                dapps_enabled,
                address_book_slot_id,
            },
        },
    )
}

pub fn finalize_balance_account_creation(
    program_id: &Pubkey,
    wallet_account: &Pubkey,
    multisig_op_account: &Pubkey,
    rent_collector_account: &Pubkey,
    account_guid_hash: BalanceAccountGuidHash,
    creation_params: BalanceAccountCreation,
) -> Instruction {
    let data = ProgramInstruction::FinalizeBalanceAccountCreation {
        account_guid_hash,
        creation_params,
    }
    .borrow()
    .pack();
    let accounts = vec![
        AccountMeta::new(*multisig_op_account, false),
        AccountMeta::new(*wallet_account, false),
        AccountMeta::new_readonly(*rent_collector_account, true),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
    ];

    Instruction {
        program_id: *program_id,
        accounts,
        data,
    }
}

pub fn init_dapp_book_update(
    program_id: &Pubkey,
    wallet_account: &Pubkey,
    multisig_op_account: &Pubkey,
    initiator_account: &Pubkey,
    update: DAppBookUpdate,
) -> Instruction {
    init_multisig_op(
        program_id,
        wallet_account,
        multisig_op_account,
        initiator_account,
        ProgramInstruction::InitDAppBookUpdate { update },
    )
}

pub fn finalize_dapp_book_update(
    program_id: &Pubkey,
    wallet_account: &Pubkey,
    multisig_op_account: &Pubkey,
    rent_collector_account: &Pubkey,
    update: DAppBookUpdate,
) -> Instruction {
    let data = ProgramInstruction::FinalizeDAppBookUpdate { update }
        .borrow()
        .pack();
    let accounts = vec![
        AccountMeta::new(*multisig_op_account, false),
        AccountMeta::new(*wallet_account, false),
        AccountMeta::new_readonly(*rent_collector_account, true),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
    ];

    Instruction {
        program_id: *program_id,
        accounts,
        data,
    }
}

pub fn init_balance_account_policy_update_instruction(
    program_id: &Pubkey,
    wallet_account: &Pubkey,
    multisig_op_account: &Pubkey,
    initiator_account: &Pubkey,
    account_guid_hash: BalanceAccountGuidHash,
    update: BalanceAccountPolicyUpdate,
) -> Instruction {
    Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new(*multisig_op_account, false),
            AccountMeta::new(*wallet_account, false),
            AccountMeta::new_readonly(*initiator_account, true),
            AccountMeta::new_readonly(sysvar::clock::id(), false),
        ],
        data: ProgramInstruction::InitBalanceAccountPolicyUpdate {
            account_guid_hash,
            update: update.clone(),
        }
        .borrow()
        .pack(),
    }
}

pub fn finalize_balance_account_policy_update_instruction(
    program_id: &Pubkey,
    wallet_account: &Pubkey,
    multisig_op_account: &Pubkey,
    rent_collector_account: &Pubkey,
    account_guid_hash: BalanceAccountGuidHash,
    update: BalanceAccountPolicyUpdate,
) -> Instruction {
    let accounts = vec![
        AccountMeta::new(*multisig_op_account, false),
        AccountMeta::new(*wallet_account, false),
        AccountMeta::new_readonly(*rent_collector_account, true),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
    ];

    Instruction {
        program_id: *program_id,
        accounts,
        data: ProgramInstruction::FinalizeBalanceAccountPolicyUpdate {
            account_guid_hash,
            update,
        }
        .borrow()
        .pack(),
    }
}

pub fn init_transfer(
    program_id: &Pubkey,
    wallet_account: &Pubkey,
    multisig_op_account: &Pubkey,
    initiator_account: &Pubkey,
    source_account: &Pubkey,
    destination_account: &Pubkey,
    account_guid_hash: BalanceAccountGuidHash,
    amount: u64,
    destination_name_hash: AddressBookEntryNameHash,
    token_mint: &Pubkey,
    fee_payer: &Pubkey,
) -> Instruction {
    let data = ProgramInstruction::InitTransfer {
        account_guid_hash,
        amount,
        destination_name_hash,
    }
    .borrow()
    .pack();

    let destination_token_account =
        spl_associated_token_account::get_associated_token_address(destination_account, token_mint);

    let accounts = vec![
        AccountMeta::new(*multisig_op_account, false),
        AccountMeta::new_readonly(*wallet_account, false),
        AccountMeta::new(*source_account, false),
        AccountMeta::new_readonly(*destination_account, false),
        AccountMeta::new_readonly(*initiator_account, true),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
        AccountMeta::new_readonly(*token_mint, false),
        AccountMeta::new(destination_token_account, false),
        AccountMeta::new(*fee_payer, true),
        AccountMeta::new_readonly(system_program::id(), false),
        AccountMeta::new_readonly(spl_token::id(), false),
        AccountMeta::new_readonly(sysvar::rent::id(), false),
        AccountMeta::new_readonly(spl_associated_token_account::id(), false),
    ];

    Instruction {
        program_id: *program_id,
        accounts,
        data,
    }
}

pub fn finalize_transfer(
    program_id: &Pubkey,
    multisig_op_account: &Pubkey,
    wallet_account: &Pubkey,
    source_account: &Pubkey,
    destination_account: &Pubkey,
    rent_collector_account: &Pubkey,
    account_guid_hash: BalanceAccountGuidHash,
    amount: u64,
    token_mint: &Pubkey,
    token_authority: Option<&Pubkey>,
) -> Instruction {
    let data = ProgramInstruction::FinalizeTransfer {
        account_guid_hash,
        amount,
        token_mint: *token_mint,
    }
    .borrow()
    .pack();
    let mut accounts = vec![
        AccountMeta::new(*multisig_op_account, false),
        AccountMeta::new_readonly(*wallet_account, false),
        AccountMeta::new(*source_account, false),
        AccountMeta::new(*destination_account, false),
        AccountMeta::new_readonly(system_program::id(), false),
        AccountMeta::new_readonly(*rent_collector_account, true),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
    ];
    if *token_mint != system_program::id() {
        // SPL
        accounts.extend_from_slice(&[
            AccountMeta::new(
                spl_associated_token_account::get_associated_token_address(
                    source_account,
                    &token_mint,
                ),
                false,
            ),
            AccountMeta::new(
                spl_associated_token_account::get_associated_token_address(
                    destination_account,
                    &token_mint,
                ),
                false,
            ),
            AccountMeta::new_readonly(spl_token::id(), false),
            AccountMeta::new_readonly(*token_authority.unwrap(), false),
        ])
    }
    Instruction {
        program_id: *program_id,
        accounts,
        data,
    }
}

pub fn init_wrap_unwrap(
    program_id: &Pubkey,
    wallet_account: &Pubkey,
    multisig_op_account: &Pubkey,
    initiator_account: &Pubkey,
    balance_account: &Pubkey,
    account_guid_hash: &BalanceAccountGuidHash,
    amount: u64,
    direction: WrapDirection,
) -> Instruction {
    let data = ProgramInstruction::InitWrapUnwrap {
        account_guid_hash: *account_guid_hash,
        amount,
        direction,
    }
    .borrow()
    .pack();

    let wrapped_sol_account = spl_associated_token_account::get_associated_token_address(
        balance_account,
        &spl_token::native_mint::id(),
    );

    let accounts = vec![
        AccountMeta::new(*multisig_op_account, false),
        AccountMeta::new_readonly(*wallet_account, false),
        AccountMeta::new(*balance_account, false),
        AccountMeta::new(wrapped_sol_account, false),
        AccountMeta::new_readonly(spl_token::native_mint::id(), false),
        AccountMeta::new_readonly(*initiator_account, true),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
        AccountMeta::new_readonly(system_program::id(), false),
        AccountMeta::new_readonly(spl_token::id(), false),
        AccountMeta::new_readonly(sysvar::rent::id(), false),
        AccountMeta::new_readonly(spl_associated_token_account::id(), false),
    ];

    Instruction {
        program_id: *program_id,
        accounts,
        data,
    }
}

pub fn finalize_wrap_unwrap(
    program_id: &Pubkey,
    multisig_op_account: &Pubkey,
    wallet_account: &Pubkey,
    balance_account: &Pubkey,
    rent_collector_account: &Pubkey,
    account_guid_hash: &BalanceAccountGuidHash,
    amount: u64,
    direction: WrapDirection,
) -> Instruction {
    let data = ProgramInstruction::FinalizeWrapUnwrap {
        account_guid_hash: *account_guid_hash,
        amount,
        direction,
    }
    .borrow()
    .pack();

    let wrapped_sol_account = spl_associated_token_account::get_associated_token_address(
        balance_account,
        &spl_token::native_mint::id(),
    );

    let accounts = vec![
        AccountMeta::new(*multisig_op_account, false),
        AccountMeta::new_readonly(*wallet_account, false),
        AccountMeta::new(*balance_account, false),
        AccountMeta::new_readonly(system_program::id(), false),
        AccountMeta::new_readonly(*rent_collector_account, true),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
        AccountMeta::new(wrapped_sol_account, false),
        AccountMeta::new_readonly(spl_token::id(), false),
    ];

    Instruction {
        program_id: *program_id,
        accounts,
        data,
    }
}

pub fn init_update_signer(
    program_id: &Pubkey,
    wallet_account: &Pubkey,
    multisig_op_account: &Pubkey,
    assistant_account: &Pubkey,
    slot_update_type: SlotUpdateType,
    slot_id: SlotId<Signer>,
    signer: Signer,
) -> Instruction {
    init_multisig_op(
        program_id,
        wallet_account,
        multisig_op_account,
        assistant_account,
        ProgramInstruction::InitUpdateSigner {
            slot_update_type,
            slot_id,
            signer,
        },
    )
}

pub fn finalize_update_signer(
    program_id: &Pubkey,
    wallet_account: &Pubkey,
    multisig_op_account: &Pubkey,
    rent_collector_account: &Pubkey,
    slot_update_type: SlotUpdateType,
    slot_id: SlotId<Signer>,
    signer: Signer,
) -> Instruction {
    let data = ProgramInstruction::FinalizeUpdateSigner {
        slot_update_type,
        slot_id,
        signer,
    }
    .borrow()
    .pack();

    let accounts = vec![
        AccountMeta::new(*multisig_op_account, false),
        AccountMeta::new(*wallet_account, false),
        AccountMeta::new_readonly(*rent_collector_account, true),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
    ];

    Instruction {
        program_id: *program_id,
        accounts,
        data,
    }
}

pub fn init_wallet_config_policy_update_instruction(
    program_id: Pubkey,
    wallet_account: Pubkey,
    multisig_op_account: Pubkey,
    initiator_account: Pubkey,
    update: &WalletConfigPolicyUpdate,
) -> Instruction {
    Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new(multisig_op_account, false),
            AccountMeta::new(wallet_account, false),
            AccountMeta::new_readonly(initiator_account, true),
            AccountMeta::new_readonly(sysvar::clock::id(), false),
        ],
        data: ProgramInstruction::InitWalletConfigPolicyUpdate {
            update: update.clone(),
        }
        .borrow()
        .pack(),
    }
}

pub fn finalize_wallet_config_policy_update_instruction(
    program_id: Pubkey,
    wallet_account: Pubkey,
    multisig_op_account: Pubkey,
    rent_collector_account: Pubkey,
    update: &WalletConfigPolicyUpdate,
) -> Instruction {
    Instruction {
        program_id,
        accounts: vec![
            AccountMeta::new(multisig_op_account, false),
            AccountMeta::new(wallet_account, false),
            AccountMeta::new_readonly(rent_collector_account, true),
            AccountMeta::new_readonly(sysvar::clock::id(), false),
        ],
        data: ProgramInstruction::FinalizeWalletConfigPolicyUpdate {
            update: update.clone(),
        }
        .borrow()
        .pack(),
    }
}

pub fn init_dapp_transaction(
    program_id: &Pubkey,
    wallet_account: &Pubkey,
    multisig_op_account: &Pubkey,
    multisig_data_account: &Pubkey,
    initiator_account: &Pubkey,
    account_guid_hash: &BalanceAccountGuidHash,
    dapp: DAppBookEntry,
    instructions: Vec<Instruction>,
) -> Instruction {
    let data = ProgramInstruction::InitDAppTransaction {
        account_guid_hash: *account_guid_hash,
        dapp,
        instructions,
    }
    .borrow()
    .pack();

    let accounts = vec![
        AccountMeta::new(*multisig_op_account, false),
        AccountMeta::new(*multisig_data_account, false),
        AccountMeta::new_readonly(*wallet_account, false),
        AccountMeta::new_readonly(*initiator_account, true),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
    ];

    Instruction {
        program_id: *program_id,
        accounts,
        data,
    }
}

pub fn finalize_dapp_transaction(
    program_id: &Pubkey,
    wallet_account: &Pubkey,
    multisig_op_account: &Pubkey,
    multisig_data_account: &Pubkey,
    balance_account: &Pubkey,
    rent_collector_account: &Pubkey,
    account_guid_hash: &BalanceAccountGuidHash,
    dapp: DAppBookEntry,
    instructions: &Vec<Instruction>,
) -> Instruction {
    let data = ProgramInstruction::FinalizeDAppTransaction {
        account_guid_hash: *account_guid_hash,
        dapp,
        instructions: instructions.clone(),
    }
    .borrow()
    .pack();

    // the accounts below are expected below in this order by finalize
    let mut accounts = vec![
        AccountMeta::new(*multisig_op_account, false),
        AccountMeta::new(*multisig_data_account, false),
        AccountMeta::new_readonly(*wallet_account, false),
        AccountMeta::new(*balance_account, false),
        AccountMeta::new(*rent_collector_account, true),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
    ];

    // we also need to include any accounts referenced by the dapp instructions, but we don't
    // want to repeat keys
    let keys_to_skip = vec![
        *multisig_op_account,
        *multisig_data_account,
        *wallet_account,
        *balance_account,
        *rent_collector_account,
        sysvar::clock::id(),
    ];

    accounts.extend(utils::unique_account_metas(&instructions, &keys_to_skip));

    Instruction {
        program_id: *program_id,
        accounts,
        data,
    }
}

pub fn init_account_settings_update(
    program_id: &Pubkey,
    wallet_account: &Pubkey,
    multisig_op_account: &Pubkey,
    initiator_account: &Pubkey,
    account_guid_hash: BalanceAccountGuidHash,
    whitelist_status: Option<BooleanSetting>,
    dapps_enabled: Option<BooleanSetting>,
) -> Instruction {
    init_multisig_op(
        program_id,
        wallet_account,
        multisig_op_account,
        initiator_account,
        ProgramInstruction::InitAccountSettingsUpdate {
            account_guid_hash,
            whitelist_enabled: whitelist_status,
            dapps_enabled,
        },
    )
}

pub fn finalize_account_settings_update(
    program_id: &Pubkey,
    wallet_account: &Pubkey,
    multisig_op_account: &Pubkey,
    rent_collector_account: &Pubkey,
    account_guid_hash: BalanceAccountGuidHash,
    whitelist_status: Option<BooleanSetting>,
    dapps_enabled: Option<BooleanSetting>,
) -> Instruction {
    let data = ProgramInstruction::FinalizeAccountSettingsUpdate {
        account_guid_hash,
        whitelist_enabled: whitelist_status,
        dapps_enabled,
    }
    .borrow()
    .pack();

    let accounts = vec![
        AccountMeta::new(*multisig_op_account, false),
        AccountMeta::new(*wallet_account, false),
        AccountMeta::new(*rent_collector_account, true),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
    ];

    Instruction {
        program_id: *program_id,
        accounts,
        data,
    }
}

pub fn init_balance_account_name_update(
    program_id: &Pubkey,
    wallet_account: &Pubkey,
    multisig_op_account: &Pubkey,
    initiator_account: &Pubkey,
    account_guid_hash: BalanceAccountGuidHash,
    account_name_hash: BalanceAccountNameHash,
) -> Instruction {
    init_multisig_op(
        program_id,
        wallet_account,
        multisig_op_account,
        initiator_account,
        ProgramInstruction::InitBalanceAccountNameUpdate {
            account_guid_hash,
            account_name_hash,
        },
    )
}

pub fn finalize_balance_account_name_update(
    program_id: &Pubkey,
    wallet_account: &Pubkey,
    multisig_op_account: &Pubkey,
    rent_collector_account: &Pubkey,
    account_guid_hash: BalanceAccountGuidHash,
    account_name_hash: BalanceAccountNameHash,
) -> Instruction {
    let data = ProgramInstruction::FinalizeBalanceAccountNameUpdate {
        account_guid_hash,
        account_name_hash,
    }
    .borrow()
    .pack();

    let accounts = vec![
        AccountMeta::new(*multisig_op_account, false),
        AccountMeta::new(*wallet_account, false),
        AccountMeta::new_readonly(*rent_collector_account, true),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
    ];

    Instruction {
        program_id: *program_id,
        accounts,
        data,
    }
}

pub fn init_address_book_update(
    program_id: &Pubkey,
    wallet_account: &Pubkey,
    multisig_op_account: &Pubkey,
    initiator_account: &Pubkey,
    add_address_book_entries: Vec<(SlotId<AddressBookEntry>, AddressBookEntry)>,
    remove_address_book_entries: Vec<(SlotId<AddressBookEntry>, AddressBookEntry)>,
    balance_account_whitelist_updates: Vec<BalanceAccountWhitelistUpdate>,
) -> Instruction {
    init_multisig_op(
        program_id,
        wallet_account,
        multisig_op_account,
        initiator_account,
        ProgramInstruction::InitAddressBookUpdate {
            update: AddressBookUpdate {
                add_address_book_entries: add_address_book_entries.clone(),
                remove_address_book_entries: remove_address_book_entries.clone(),
                balance_account_whitelist_updates: balance_account_whitelist_updates.clone(),
            },
        },
    )
}

pub fn finalize_address_book_update(
    program_id: &Pubkey,
    wallet_account: &Pubkey,
    multisig_op_account: &Pubkey,
    rent_collector_account: &Pubkey,
    update: AddressBookUpdate,
) -> Instruction {
    let data = ProgramInstruction::FinalizeAddressBookUpdate { update }
        .borrow()
        .pack();
    let accounts = vec![
        AccountMeta::new(*multisig_op_account, false),
        AccountMeta::new(*wallet_account, false),
        AccountMeta::new_readonly(*rent_collector_account, true),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
    ];

    Instruction {
        program_id: *program_id,
        accounts,
        data,
    }
}
