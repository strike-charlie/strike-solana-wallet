use solana_program::account_info::{next_account_info, AccountInfo};
use solana_program::entrypoint::ProgramResult;
use solana_program::hash::Hash;
use solana_program::instruction::{AccountMeta, Instruction};
use solana_program::program::{invoke, invoke_signed};
use solana_program::program_error::ProgramError;
use solana_program::program_pack::{IsInitialized, Pack};
use solana_program::pubkey::Pubkey;
use solana_program::system_instruction;
use solana_program::system_program;
use solana_program::sysvar::Sysvar;
use solana_program::{msg, sysvar};
use spl_associated_token_account::get_associated_token_address;
use spl_token::id as SPL_TOKEN_ID;
use spl_token::instruction as spl_instruction;
use spl_token::state::{Account as SPLAccount, Account};

use crate::error::WalletError;
use crate::handlers::dapp_transaction_handler;
use crate::handlers::utils::{
    finalize_multisig_op, get_clock_from_next_account, next_program_account_info,
    start_multisig_config_op, start_multisig_transfer_op, validate_balance_account_and_get_seed,
};
use crate::handlers::{account_settings_update_handler, wallet_config_policy_update_handler};
use crate::instruction::{BalanceAccountUpdate, ProgramInstruction, WalletUpdate};
use crate::model::address_book::AddressBookEntryNameHash;
use crate::model::balance_account::BalanceAccountGuidHash;
use crate::model::multisig_op::{
    ApprovalDisposition, MultisigOp, MultisigOpParams, SlotUpdateType, WrapDirection,
};
use crate::model::signer::Signer;
use crate::model::wallet::Wallet;
use crate::utils::SlotId;
use solana_program::rent::Rent;

pub struct Processor;
impl Processor {
    pub fn process(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        instruction_data: &[u8],
    ) -> ProgramResult {
        let instruction = ProgramInstruction::unpack(instruction_data)?;

        match instruction {
            ProgramInstruction::InitWallet { update } => {
                Self::handle_init_wallet(program_id, accounts, &update)
            }

            ProgramInstruction::InitWalletUpdate { update } => {
                Self::handle_init_wallet_update(program_id, accounts, &update)
            }

            ProgramInstruction::FinalizeWalletUpdate { update } => {
                Self::handle_finalize_wallet_update(program_id, accounts, &update)
            }

            ProgramInstruction::InitWalletConfigPolicyUpdate { update } => {
                wallet_config_policy_update_handler::init(program_id, accounts, &update)
            }

            ProgramInstruction::FinalizeWalletConfigPolicyUpdate { update } => {
                wallet_config_policy_update_handler::finalize(program_id, accounts, &update)
            }

            ProgramInstruction::InitBalanceAccountCreation {
                account_guid_hash,
                update,
            } => Self::handle_init_balance_account_creation(
                program_id,
                accounts,
                &account_guid_hash,
                &update,
            ),

            ProgramInstruction::FinalizeBalanceAccountCreation {
                account_guid_hash,
                update,
            } => Self::handle_finalize_balance_account_creation(
                program_id,
                accounts,
                &account_guid_hash,
                &update,
            ),

            ProgramInstruction::InitBalanceAccountUpdate {
                account_guid_hash,
                update,
            } => Self::handle_init_balance_account_update(
                program_id,
                accounts,
                &account_guid_hash,
                &update,
            ),

            ProgramInstruction::FinalizeBalanceAccountUpdate {
                account_guid_hash,
                update,
            } => Self::handle_finalize_balance_account_update(
                program_id,
                accounts,
                &account_guid_hash,
                &update,
            ),

            ProgramInstruction::InitTransfer {
                account_guid_hash,
                amount,
                destination_name_hash,
            } => Self::handle_init_transfer(
                program_id,
                &accounts,
                &account_guid_hash,
                amount,
                &destination_name_hash,
            ),

            ProgramInstruction::FinalizeTransfer {
                account_guid_hash,
                amount,
                token_mint,
            } => Self::handle_finalize_transfer(
                program_id,
                &accounts,
                &account_guid_hash,
                amount,
                token_mint,
            ),

            ProgramInstruction::SetApprovalDisposition {
                disposition,
                params_hash,
            } => Self::handle_approval_disposition(program_id, &accounts, disposition, params_hash),

            ProgramInstruction::InitWrapUnwrap {
                account_guid_hash,
                amount,
                direction,
            } => Self::handle_init_wrap_unwrap(
                program_id,
                &accounts,
                &account_guid_hash,
                amount,
                direction,
            ),

            ProgramInstruction::FinalizeWrapUnwrap {
                account_guid_hash,
                amount,
                direction,
            } => Self::handle_finalize_wrap_unwrap(
                program_id,
                &accounts,
                &account_guid_hash,
                amount,
                direction,
            ),

            ProgramInstruction::InitUpdateSigner {
                slot_update_type,
                slot_id,
                signer,
            } => Self::handle_init_update_signer(
                program_id,
                &accounts,
                slot_update_type,
                slot_id,
                signer,
            ),

            ProgramInstruction::FinalizeUpdateSigner {
                slot_update_type,
                slot_id,
                signer,
            } => Self::handle_finalize_update_signer(
                program_id,
                &accounts,
                slot_update_type,
                slot_id,
                signer,
            ),

            ProgramInstruction::InitDAppTransaction {
                ref account_guid_hash,
                instructions,
            } => dapp_transaction_handler::init(
                program_id,
                accounts,
                account_guid_hash,
                instructions,
            ),

            ProgramInstruction::FinalizeDAppTransaction {
                ref account_guid_hash,
                ref instructions,
            } => dapp_transaction_handler::finalize(
                program_id,
                accounts,
                account_guid_hash,
                instructions,
            ),

            ProgramInstruction::InitAccountSettingsUpdate {
                account_guid_hash,
                whitelist_enabled,
                dapps_enabled,
            } => account_settings_update_handler::init(
                program_id,
                &accounts,
                &account_guid_hash,
                whitelist_enabled,
                dapps_enabled,
            ),

            ProgramInstruction::FinalizeAccountSettingsUpdate {
                account_guid_hash,
                whitelist_enabled,
                dapps_enabled,
            } => account_settings_update_handler::finalize(
                program_id,
                &accounts,
                &account_guid_hash,
                whitelist_enabled,
                dapps_enabled,
            ),
        }
    }

    fn handle_init_wallet(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        update: &WalletUpdate,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let wallet_account_info = next_program_account_info(accounts_iter, program_id)?;
        let assistant_account_info = next_account_info(accounts_iter)?;

        let mut wallet = Wallet::unpack_unchecked(&wallet_account_info.data.borrow())?;

        if wallet.is_initialized() {
            return Err(ProgramError::AccountAlreadyInitialized);
        }

        wallet.is_initialized = true;
        wallet.assistant = Signer {
            key: *assistant_account_info.key,
        };
        wallet.update(update)?;
        Wallet::pack(wallet, &mut wallet_account_info.data.borrow_mut())?;

        Ok(())
    }

    fn handle_init_wallet_update(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        update: &WalletUpdate,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let multisig_op_account_info = next_program_account_info(accounts_iter, program_id)?;
        let wallet_account_info = next_program_account_info(accounts_iter, program_id)?;
        let initiator_account_info = next_account_info(accounts_iter)?;
        let clock = get_clock_from_next_account(accounts_iter)?;

        let wallet = Wallet::unpack(&wallet_account_info.data.borrow())?;
        wallet.validate_config_initiator(initiator_account_info)?;
        wallet.validate_update(update)?;

        start_multisig_config_op(
            &multisig_op_account_info,
            &wallet,
            clock,
            MultisigOpParams::UpdateWallet {
                wallet_address: *wallet_account_info.key,
                update: update.clone(),
            },
        )
    }

    fn handle_finalize_wallet_update(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        update: &WalletUpdate,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let multisig_op_account_info = next_program_account_info(accounts_iter, program_id)?;
        let wallet_account_info = next_program_account_info(accounts_iter, program_id)?;
        let account_to_return_rent_to = next_account_info(accounts_iter)?;
        let clock = get_clock_from_next_account(accounts_iter)?;

        finalize_multisig_op(
            &multisig_op_account_info,
            &account_to_return_rent_to,
            clock,
            MultisigOpParams::UpdateWallet {
                wallet_address: *wallet_account_info.key,
                update: update.clone(),
            },
            || -> ProgramResult {
                let mut wallet = Wallet::unpack(&wallet_account_info.data.borrow_mut())?;
                wallet.update(update)?;
                Wallet::pack(wallet, &mut wallet_account_info.data.borrow_mut())?;
                Ok(())
            },
        )
    }

    fn handle_init_balance_account_creation(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        account_guid_hash: &BalanceAccountGuidHash,
        update: &BalanceAccountUpdate,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let multisig_op_account_info = next_program_account_info(accounts_iter, program_id)?;
        let wallet_account_info = next_program_account_info(accounts_iter, program_id)?;
        let initiator_account_info = next_account_info(accounts_iter)?;
        let clock = get_clock_from_next_account(accounts_iter)?;

        let wallet = Wallet::unpack(&wallet_account_info.data.borrow())?;
        wallet.validate_config_initiator(initiator_account_info)?;
        wallet.validate_add_balance_account(account_guid_hash, update)?;

        start_multisig_config_op(
            &multisig_op_account_info,
            &wallet,
            clock,
            MultisigOpParams::CreateBalanceAccount {
                account_guid_hash: *account_guid_hash,
                wallet_address: *wallet_account_info.key,
                update: update.clone(),
            },
        )
    }

    fn handle_finalize_balance_account_creation(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        account_guid_hash: &BalanceAccountGuidHash,
        update: &BalanceAccountUpdate,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let multisig_op_account_info = next_program_account_info(accounts_iter, program_id)?;
        let wallet_account_info = next_program_account_info(accounts_iter, program_id)?;
        let rent_collector_account_info = next_account_info(accounts_iter)?;
        let clock = get_clock_from_next_account(accounts_iter)?;

        finalize_multisig_op(
            &multisig_op_account_info,
            &rent_collector_account_info,
            clock,
            MultisigOpParams::CreateBalanceAccount {
                account_guid_hash: *account_guid_hash,
                wallet_address: *wallet_account_info.key,
                update: update.clone(),
            },
            || -> ProgramResult {
                let mut wallet = Wallet::unpack(&wallet_account_info.data.borrow())?;
                wallet.add_balance_account(account_guid_hash, update)?;
                Wallet::pack(wallet, &mut wallet_account_info.data.borrow_mut())?;
                Ok(())
            },
        )
    }

    fn handle_init_balance_account_update(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        account_guid_hash: &BalanceAccountGuidHash,
        update: &BalanceAccountUpdate,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let multisig_op_account_info = next_program_account_info(accounts_iter, program_id)?;
        let wallet_account_info = next_program_account_info(accounts_iter, program_id)?;
        let initiator_account_info = next_account_info(accounts_iter)?;
        let clock = get_clock_from_next_account(accounts_iter)?;

        let wallet = Wallet::unpack(&wallet_account_info.data.borrow())?;
        wallet.validate_config_initiator(initiator_account_info)?;
        wallet.validate_balance_account_update(account_guid_hash, update)?;

        start_multisig_config_op(
            &multisig_op_account_info,
            &wallet,
            clock,
            MultisigOpParams::UpdateBalanceAccount {
                wallet_address: *wallet_account_info.key,
                account_guid_hash: *account_guid_hash,
                update: update.clone(),
            },
        )
    }

    fn handle_finalize_balance_account_update(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        account_guid_hash: &BalanceAccountGuidHash,
        update: &BalanceAccountUpdate,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let multisig_op_account_info = next_program_account_info(accounts_iter, program_id)?;
        let wallet_account_info = next_program_account_info(accounts_iter, program_id)?;
        let rent_collector_account_info = next_account_info(accounts_iter)?;
        let clock = get_clock_from_next_account(accounts_iter)?;

        finalize_multisig_op(
            &multisig_op_account_info,
            &rent_collector_account_info,
            clock,
            MultisigOpParams::UpdateBalanceAccount {
                account_guid_hash: *account_guid_hash,
                wallet_address: *wallet_account_info.key,
                update: update.clone(),
            },
            || -> ProgramResult {
                let mut wallet = Wallet::unpack(&wallet_account_info.data.borrow())?;
                wallet.update_balance_account(account_guid_hash, update)?;
                Wallet::pack(wallet, &mut wallet_account_info.data.borrow_mut())?;
                Ok(())
            },
        )
    }

    fn handle_init_transfer(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        account_guid_hash: &BalanceAccountGuidHash,
        amount: u64,
        destination_name_hash: &AddressBookEntryNameHash,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let multisig_op_account_info = next_program_account_info(accounts_iter, program_id)?;
        let wallet_account_info = next_program_account_info(accounts_iter, program_id)?;
        let source_account = next_account_info(accounts_iter)?;
        let destination_account = next_account_info(accounts_iter)?;
        let initiator_account_info = next_account_info(accounts_iter)?;
        let clock = get_clock_from_next_account(accounts_iter)?;
        let token_mint = next_account_info(accounts_iter)?;
        let destination_token_account = next_account_info(accounts_iter)?;

        let wallet = Wallet::unpack(&wallet_account_info.data.borrow())?;
        let balance_account = wallet.get_balance_account(account_guid_hash)?;

        if !wallet.destination_allowed(
            balance_account,
            destination_account.key,
            destination_name_hash,
        )? {
            msg!("Destination account is not whitelisted");
            return Err(WalletError::DestinationNotAllowed.into());
        }

        wallet.validate_transfer_initiator(balance_account, initiator_account_info)?;

        if *token_mint.key != Pubkey::default()
            && *destination_token_account.owner == Pubkey::default()
        {
            // we need to create the destination token account (if it had been created already
            // it would be owned by the Token program).
            // frst check if the source account has sufficient funds to create it
            let rent = Rent::get()?;
            if rent.is_exempt(source_account.lamports(), Account::LEN) {
                let (source_account_pda, bump_seed) =
                    Pubkey::find_program_address(&[&account_guid_hash.to_bytes()], program_id);
                if &source_account_pda != source_account.key {
                    return Err(WalletError::InvalidSourceAccount.into());
                }
                invoke_signed(
                    &Instruction {
                        program_id: spl_associated_token_account::id(),
                        accounts: vec![
                            AccountMeta::new(source_account_pda, true),
                            AccountMeta::new(*destination_token_account.key, false),
                            AccountMeta::new_readonly(*destination_account.key, false),
                            AccountMeta::new_readonly(*token_mint.key, false),
                            AccountMeta::new_readonly(solana_program::system_program::id(), false),
                            AccountMeta::new_readonly(spl_token::id(), false),
                            AccountMeta::new_readonly(sysvar::rent::id(), false),
                        ],
                        data: vec![],
                    },
                    accounts,
                    &[&[&account_guid_hash.to_bytes(), &[bump_seed]]],
                )?;
            } else {
                let fee_payer_account = next_account_info(accounts_iter)?;
                invoke(
                    &Instruction {
                        program_id: spl_associated_token_account::id(),
                        accounts: vec![
                            AccountMeta::new(*fee_payer_account.key, true),
                            AccountMeta::new(*destination_token_account.key, false),
                            AccountMeta::new_readonly(*destination_account.key, false),
                            AccountMeta::new_readonly(*token_mint.key, false),
                            AccountMeta::new_readonly(solana_program::system_program::id(), false),
                            AccountMeta::new_readonly(spl_token::id(), false),
                            AccountMeta::new_readonly(sysvar::rent::id(), false),
                        ],
                        data: vec![],
                    },
                    accounts,
                )?;
            }
        }

        start_multisig_transfer_op(
            &multisig_op_account_info,
            &wallet,
            &balance_account,
            clock,
            MultisigOpParams::Transfer {
                wallet_address: *wallet_account_info.key,
                account_guid_hash: *account_guid_hash,
                destination: *destination_account.key,
                amount,
                token_mint: *token_mint.key,
            },
        )
    }

    fn handle_finalize_transfer(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        account_guid_hash: &BalanceAccountGuidHash,
        amount: u64,
        token_mint: Pubkey,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let multisig_op_account_info = next_program_account_info(accounts_iter, program_id)?;
        let wallet_account_info = next_program_account_info(accounts_iter, program_id)?;
        let source_account = next_account_info(accounts_iter)?;
        let destination_account = next_account_info(accounts_iter)?;
        let system_program_account = next_account_info(accounts_iter)?;
        let rent_collector_account_info = next_account_info(accounts_iter)?;
        let clock = get_clock_from_next_account(accounts_iter)?;

        let is_spl = token_mint.to_bytes() != [0; 32];

        if system_program_account.key != &system_program::id() {
            return Err(ProgramError::InvalidArgument);
        }

        finalize_multisig_op(
            &multisig_op_account_info,
            &rent_collector_account_info,
            clock,
            MultisigOpParams::Transfer {
                wallet_address: *wallet_account_info.key,
                account_guid_hash: *account_guid_hash,
                destination: *destination_account.key,
                amount,
                token_mint,
            },
            || -> ProgramResult {
                let bump_seed = validate_balance_account_and_get_seed(
                    source_account,
                    account_guid_hash,
                    program_id,
                )?;
                if is_spl {
                    let source_token_account = next_account_info(accounts_iter)?;
                    let source_token_account_key =
                        get_associated_token_address(source_account.key, &token_mint);
                    if *source_token_account.key != source_token_account_key {
                        return Err(WalletError::InvalidSourceTokenAccount.into());
                    }
                    let source_token_account_data =
                        SPLAccount::unpack(&source_token_account.data.borrow())?;
                    if source_token_account_data.amount < amount {
                        msg!(
                            "Source token account only has {} tokens of {} requested",
                            source_token_account_data.amount,
                            amount
                        );
                        return Err(WalletError::InsufficientBalance.into());
                    }
                    let destination_token_account = next_account_info(accounts_iter)?;
                    let destination_token_account_key =
                        get_associated_token_address(&destination_account.key, &token_mint);
                    if *destination_token_account.key != destination_token_account_key {
                        return Err(WalletError::InvalidDestinationTokenAccount.into());
                    }

                    let spl_token_program = next_account_info(accounts_iter)?;
                    let token_mint_authority = next_account_info(accounts_iter)?;

                    invoke_signed(
                        &spl_instruction::transfer(
                            &SPL_TOKEN_ID(),
                            &source_token_account_key,
                            &destination_token_account_key,
                            source_account.key,
                            &[],
                            amount,
                        )?,
                        &[
                            source_token_account.clone(),
                            destination_token_account.clone(),
                            source_account.clone(),
                            destination_account.clone(),
                            token_mint_authority.clone(),
                            spl_token_program.clone(),
                        ],
                        &[&[&account_guid_hash.to_bytes(), &[bump_seed]]],
                    )?;
                } else {
                    if source_account.lamports() < amount {
                        msg!(
                            "Source account only has {} lamports of {} requested",
                            source_account.lamports(),
                            amount
                        );
                        return Err(WalletError::InsufficientBalance.into());
                    }

                    Self::transfer_sol_checked(
                        source_account.clone(),
                        account_guid_hash,
                        bump_seed,
                        system_program_account.clone(),
                        destination_account.clone(),
                        amount,
                    )?;
                }
                Ok(())
            },
        )
    }

    fn handle_init_wrap_unwrap(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        account_guid_hash: &BalanceAccountGuidHash,
        amount: u64,
        direction: WrapDirection,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let multisig_op_account_info = next_program_account_info(accounts_iter, program_id)?;
        let wallet_account_info = next_program_account_info(accounts_iter, program_id)?;
        let balance_account_info = next_account_info(accounts_iter)?;
        let wrapped_sol_account_info = next_account_info(accounts_iter)?;
        let native_mint_account_info = next_account_info(accounts_iter)?;
        if *native_mint_account_info.key != spl_token::native_mint::id() {
            msg!("Invalid native mint account set");
            return Err(ProgramError::InvalidAccountData);
        }

        let initiator_account = next_account_info(accounts_iter)?;
        let clock = get_clock_from_next_account(accounts_iter)?;

        let wallet = Wallet::unpack(&wallet_account_info.data.borrow())?;
        let balance_account = wallet.get_balance_account(&account_guid_hash)?;

        wallet.validate_transfer_initiator(balance_account, initiator_account)?;

        if direction == WrapDirection::WRAP && *wrapped_sol_account_info.owner == Pubkey::default()
        {
            // we need to create the wrapped SOL account (if it had been created already
            // it would be owned by the Token program). Since this is an attempt to wrap
            // SOL, it stands to reason they have some SOL in their account, so we assume
            // they have enough to create this account (if they don't, it will just fail)
            let bump_seed = validate_balance_account_and_get_seed(
                balance_account_info,
                account_guid_hash,
                program_id,
            )?;
            invoke_signed(
                &Instruction {
                    program_id: spl_associated_token_account::id(),
                    accounts: vec![
                        AccountMeta::new(*balance_account_info.key, true),
                        AccountMeta::new(*wrapped_sol_account_info.key, false),
                        AccountMeta::new_readonly(*balance_account_info.key, false),
                        AccountMeta::new_readonly(*native_mint_account_info.key, false),
                        AccountMeta::new_readonly(solana_program::system_program::id(), false),
                        AccountMeta::new_readonly(spl_token::id(), false),
                        AccountMeta::new_readonly(sysvar::rent::id(), false),
                    ],
                    data: vec![],
                },
                accounts,
                &[&[&account_guid_hash.to_bytes(), &[bump_seed]]],
            )?;
        }

        start_multisig_transfer_op(
            &multisig_op_account_info,
            &wallet,
            &balance_account,
            clock,
            MultisigOpParams::Wrap {
                wallet_address: *wallet_account_info.key,
                account_guid_hash: *account_guid_hash,
                amount,
                direction,
            },
        )
    }

    fn handle_finalize_wrap_unwrap(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        account_guid_hash: &BalanceAccountGuidHash,
        amount: u64,
        direction: WrapDirection,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let multisig_op_account_info = next_program_account_info(accounts_iter, program_id)?;
        let wallet_account_info = next_program_account_info(accounts_iter, program_id)?;
        let balance_account_info = next_account_info(accounts_iter)?;
        let system_program_account_info = next_account_info(accounts_iter)?;
        let rent_collector_account_info = next_account_info(accounts_iter)?;
        let clock = get_clock_from_next_account(accounts_iter)?;
        let wrapped_sol_account_info = next_account_info(accounts_iter)?;

        if system_program_account_info.key != &system_program::id() {
            return Err(ProgramError::InvalidArgument);
        }

        finalize_multisig_op(
            &multisig_op_account_info,
            &rent_collector_account_info,
            clock,
            MultisigOpParams::Wrap {
                wallet_address: *wallet_account_info.key,
                account_guid_hash: *account_guid_hash,
                amount,
                direction,
            },
            || -> ProgramResult {
                let bump_seed = validate_balance_account_and_get_seed(
                    balance_account_info,
                    account_guid_hash,
                    program_id,
                )?;

                let wrapped_sol_account_key = get_associated_token_address(
                    balance_account_info.key,
                    &spl_token::native_mint::id(),
                );
                if *wrapped_sol_account_info.key != wrapped_sol_account_key {
                    return Err(WalletError::InvalidSourceTokenAccount.into());
                }

                if direction == WrapDirection::WRAP {
                    Self::transfer_sol_checked(
                        balance_account_info.clone(),
                        account_guid_hash,
                        bump_seed,
                        system_program_account_info.clone(),
                        wrapped_sol_account_info.clone(),
                        amount,
                    )?;
                } else {
                    let wrapped_sol_account_data =
                        SPLAccount::unpack(&wrapped_sol_account_info.data.borrow())?;
                    if wrapped_sol_account_data.amount < amount {
                        msg!(
                            "Wrapped SOL account only has {} lamports of {} requested",
                            wrapped_sol_account_data.amount,
                            amount
                        );
                        return Err(WalletError::InsufficientBalance.into());
                    }

                    // the only way to transfer lamports out of a token account is to close it, so we first
                    // close it and then transfer back whatever is remaining
                    let remaining = wrapped_sol_account_info
                        .lamports()
                        .checked_sub(amount)
                        .ok_or(WalletError::AmountOverflow)?;

                    invoke_signed(
                        &spl_token::instruction::close_account(
                            &spl_token::id(),
                            &wrapped_sol_account_info.key,
                            &balance_account_info.key,
                            &balance_account_info.key,
                            &[],
                        )?,
                        &[
                            wrapped_sol_account_info.clone(),
                            balance_account_info.clone(),
                        ],
                        &[&[&account_guid_hash.to_bytes(), &[bump_seed]]],
                    )?;

                    Self::transfer_sol_checked(
                        balance_account_info.clone(),
                        account_guid_hash,
                        bump_seed,
                        system_program_account_info.clone(),
                        wrapped_sol_account_info.clone(),
                        remaining,
                    )?;
                }

                invoke(
                    &spl_token::instruction::sync_native(
                        &spl_token::id(),
                        &wrapped_sol_account_key,
                    )?,
                    &[wrapped_sol_account_info.clone()],
                )?;
                Ok(())
            },
        )
    }

    fn handle_approval_disposition(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        disposition: ApprovalDisposition,
        params_hash: Hash,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let multisig_op_account_info = next_program_account_info(accounts_iter, program_id)?;
        let signer_account_info = next_account_info(accounts_iter)?;
        let clock = get_clock_from_next_account(accounts_iter)?;

        let mut multisig_op = MultisigOp::unpack(&multisig_op_account_info.data.borrow())?;

        if params_hash != multisig_op.params_hash {
            return Err(WalletError::InvalidSignature.into());
        }

        multisig_op.validate_and_record_approval_disposition(
            &signer_account_info,
            disposition,
            &clock,
        )?;
        MultisigOp::pack(multisig_op, &mut multisig_op_account_info.data.borrow_mut())?;

        Ok(())
    }

    fn handle_init_update_signer(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        slot_update_type: SlotUpdateType,
        slot_id: SlotId<Signer>,
        signer: Signer,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let multisig_op_account_info = next_program_account_info(accounts_iter, program_id)?;
        let wallet_account_info = next_program_account_info(accounts_iter, program_id)?;
        let initiator_account_info = next_account_info(accounts_iter)?;
        let clock = get_clock_from_next_account(accounts_iter)?;

        let wallet = Wallet::unpack(&wallet_account_info.data.borrow())?;
        wallet.validate_config_initiator(initiator_account_info)?;
        match slot_update_type {
            SlotUpdateType::SetIfEmpty => wallet.validate_add_signer((slot_id, signer))?,
            SlotUpdateType::Clear => wallet.validate_remove_signer((slot_id, signer))?,
        }

        start_multisig_config_op(
            &multisig_op_account_info,
            &wallet,
            clock,
            MultisigOpParams::UpdateSigner {
                wallet_address: *wallet_account_info.key,
                slot_update_type,
                slot_id,
                signer,
            },
        )
    }

    fn handle_finalize_update_signer(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        slot_update_type: SlotUpdateType,
        slot_id: SlotId<Signer>,
        signer: Signer,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let multisig_op_account_info = next_program_account_info(accounts_iter, program_id)?;
        let wallet_account_info = next_program_account_info(accounts_iter, program_id)?;
        let account_to_return_rent_to = next_account_info(accounts_iter)?;
        let clock = get_clock_from_next_account(accounts_iter)?;

        finalize_multisig_op(
            &multisig_op_account_info,
            &account_to_return_rent_to,
            clock,
            MultisigOpParams::UpdateSigner {
                wallet_address: *wallet_account_info.key,
                slot_update_type,
                slot_id,
                signer,
            },
            || -> ProgramResult {
                let mut wallet = Wallet::unpack(&wallet_account_info.data.borrow_mut())?;
                match slot_update_type {
                    SlotUpdateType::SetIfEmpty => wallet.add_signer((slot_id, signer))?,
                    SlotUpdateType::Clear => wallet.remove_signer((slot_id, signer))?,
                }
                Wallet::pack(wallet, &mut wallet_account_info.data.borrow_mut())?;
                Ok(())
            },
        )
    }

    fn transfer_sol_checked<'a>(
        balance_account: AccountInfo<'a>,
        account_guid_hash: &BalanceAccountGuidHash,
        bump_seed: u8,
        system_program_account: AccountInfo<'a>,
        to: AccountInfo<'a>,
        lamports: u64,
    ) -> ProgramResult {
        if balance_account.lamports() < lamports {
            msg!(
                "Account only has {} lamports of {} requested",
                balance_account.lamports(),
                lamports
            );
            return Err(WalletError::InsufficientBalance.into());
        }
        let instruction = &system_instruction::transfer(balance_account.key, to.key, lamports);
        invoke_signed(
            instruction,
            &[balance_account, to, system_program_account],
            &[&[&account_guid_hash.to_bytes(), &[bump_seed]]],
        )
    }
}
