use std::slice::Iter;
use std::time::Duration;

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
use solana_program::sysvar::{is_sysvar_id, Sysvar};
use solana_program::{msg, sysvar};
use spl_associated_token_account::get_associated_token_address;
use spl_token::id as SPL_TOKEN_ID;
use spl_token::instruction as spl_instruction;
use spl_token::state::{Account as SPLAccount, Account};

use crate::error::WalletError;
use crate::instruction::{ProgramConfigUpdate, ProgramInstruction, WalletConfigUpdate};
use crate::model::multisig_op::{ApprovalDisposition, MultisigOp, MultisigOpParams, WrapDirection};
use crate::model::program_config::ProgramConfig;
use crate::model::signer::Signer;
use solana_program::clock::Clock;
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
            ProgramInstruction::Init { config_update } => {
                Self::handle_init(program_id, accounts, &config_update)
            }

            ProgramInstruction::InitConfigUpdate { config_update } => {
                Self::handle_init_config_update(program_id, accounts, &config_update)
            }

            ProgramInstruction::FinalizeConfigUpdate { config_update } => {
                Self::handle_finalize_config_update(program_id, accounts, &config_update)
            }

            ProgramInstruction::InitWalletCreation {
                wallet_guid_hash,
                config_update,
            } => Self::handle_init_wallet_creation(
                program_id,
                accounts,
                wallet_guid_hash,
                &config_update,
            ),

            ProgramInstruction::FinalizeWalletCreation {
                wallet_guid_hash,
                config_update,
            } => Self::handle_finalize_wallet_creation(
                program_id,
                accounts,
                &wallet_guid_hash,
                &config_update,
            ),

            ProgramInstruction::InitWalletConfigUpdate {
                wallet_guid_hash,
                config_update,
            } => Self::handle_init_wallet_config_update(
                program_id,
                accounts,
                &wallet_guid_hash,
                &config_update,
            ),

            ProgramInstruction::FinalizeWalletConfigUpdate {
                wallet_guid_hash,
                config_update,
            } => Self::handle_finalize_wallet_config_update(
                program_id,
                accounts,
                &wallet_guid_hash,
                &config_update,
            ),

            ProgramInstruction::InitTransfer {
                wallet_guid_hash,
                amount,
                destination_name_hash,
            } => Self::handle_init_transfer(
                program_id,
                &accounts,
                &wallet_guid_hash,
                amount,
                &destination_name_hash,
            ),

            ProgramInstruction::FinalizeTransfer {
                wallet_guid_hash,
                amount,
                token_mint,
            } => Self::handle_finalize_transfer(
                program_id,
                &accounts,
                wallet_guid_hash,
                amount,
                token_mint,
            ),

            ProgramInstruction::SetApprovalDisposition {
                disposition,
                params_hash,
            } => Self::handle_approval_disposition(program_id, &accounts, disposition, params_hash),

            ProgramInstruction::InitWrapUnwrap {
                wallet_guid_hash,
                amount,
                direction,
            } => Self::handle_init_wrap_unwrap(
                program_id,
                &accounts,
                wallet_guid_hash,
                amount,
                direction,
            ),

            ProgramInstruction::FinalizeWrapUnwrap {
                wallet_guid_hash,
                amount,
                direction,
            } => Self::handle_finalize_wrap_unwrap(
                program_id,
                &accounts,
                wallet_guid_hash,
                amount,
                direction,
            ),
        }
    }

    fn handle_init(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        config_update: &ProgramConfigUpdate,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let program_config_account_info =
            Self::next_program_account_info(accounts_iter, program_id)?;
        let assistant_account_info = next_account_info(accounts_iter)?;

        let mut program_config =
            ProgramConfig::unpack_unchecked(&program_config_account_info.data.borrow())?;

        if program_config.is_initialized() {
            return Err(ProgramError::AccountAlreadyInitialized);
        }

        program_config.is_initialized = true;
        program_config.assistant = Signer {
            key: *assistant_account_info.key,
        };
        program_config.update(config_update)?;
        ProgramConfig::pack(
            program_config,
            &mut program_config_account_info.data.borrow_mut(),
        )?;

        Ok(())
    }

    fn handle_init_config_update(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        config_update: &ProgramConfigUpdate,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let multisig_op_account_info = Self::next_program_account_info(accounts_iter, program_id)?;
        let program_config_account_info =
            Self::next_program_account_info(accounts_iter, program_id)?;
        let initiator_account_info = next_account_info(accounts_iter)?;
        let clock = Self::get_clock_from_next_account(accounts_iter)?;

        let program_config = ProgramConfig::unpack(&program_config_account_info.data.borrow())?;
        program_config.validate_config_initiator(initiator_account_info)?;
        program_config.validate_update(config_update)?;

        let mut multisig_op =
            MultisigOp::unpack_unchecked(&multisig_op_account_info.data.borrow())?;

        multisig_op.init(
            program_config.get_config_approvers_keys(),
            program_config.approvals_required_for_config,
            clock.unix_timestamp,
            Self::calculate_expires(
                clock.unix_timestamp,
                program_config.approval_timeout_for_config,
            )?,
            MultisigOpParams::UpdateProgramConfig {
                program_config_address: *program_config_account_info.key,
                config_update: config_update.clone(),
            },
        )?;
        MultisigOp::pack(multisig_op, &mut multisig_op_account_info.data.borrow_mut())?;

        Ok(())
    }

    fn handle_finalize_config_update(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        config_update: &ProgramConfigUpdate,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let multisig_op_account_info = Self::next_program_account_info(accounts_iter, program_id)?;
        let program_config_account_info =
            Self::next_program_account_info(accounts_iter, program_id)?;
        let account_to_return_rent_to = next_account_info(accounts_iter)?;
        let clock = Self::get_clock_from_next_account(accounts_iter)?;

        if !account_to_return_rent_to.is_signer {
            return Err(ProgramError::MissingRequiredSignature);
        }

        let multisig_op = MultisigOp::unpack(&multisig_op_account_info.data.borrow())?;

        let expected_params = MultisigOpParams::UpdateProgramConfig {
            program_config_address: *program_config_account_info.key,
            config_update: config_update.clone(),
        };

        if multisig_op.approved(&expected_params, &clock)? {
            let mut program_config =
                ProgramConfig::unpack(&program_config_account_info.data.borrow_mut())?;
            program_config.update(config_update)?;
            ProgramConfig::pack(
                program_config,
                &mut program_config_account_info.data.borrow_mut(),
            )?;
        }

        Self::collect_remaining_balance(&multisig_op_account_info, &account_to_return_rent_to)?;

        Ok(())
    }

    fn handle_init_wallet_creation(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        wallet_guid_hash: [u8; 32],
        config_update: &WalletConfigUpdate,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let multisig_op_account_info = Self::next_program_account_info(accounts_iter, program_id)?;
        let program_config_account_info =
            Self::next_program_account_info(accounts_iter, program_id)?;
        let initiator_account_info = next_account_info(accounts_iter)?;
        let clock = Self::get_clock_from_next_account(accounts_iter)?;

        let mut multisig_op =
            MultisigOp::unpack_unchecked(&multisig_op_account_info.data.borrow())?;
        let program_config = ProgramConfig::unpack(&program_config_account_info.data.borrow())?;
        program_config.validate_config_initiator(initiator_account_info)?;
        program_config.validate_add_wallet_config(&wallet_guid_hash, config_update)?;

        multisig_op.init(
            program_config.get_config_approvers_keys(),
            program_config.approvals_required_for_config,
            clock.unix_timestamp,
            Self::calculate_expires(
                clock.unix_timestamp,
                program_config.approval_timeout_for_config,
            )?,
            MultisigOpParams::CreateWallet {
                wallet_guid_hash,
                program_config_address: *program_config_account_info.key,
                config_update: config_update.clone(),
            },
        )?;
        MultisigOp::pack(multisig_op, &mut multisig_op_account_info.data.borrow_mut())?;

        Ok(())
    }

    fn handle_finalize_wallet_creation(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        wallet_guid_hash: &[u8; 32],
        config_update: &WalletConfigUpdate,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let multisig_op_account_info = Self::next_program_account_info(accounts_iter, program_id)?;
        let program_config_account_info =
            Self::next_program_account_info(accounts_iter, program_id)?;
        let rent_collector_account_info = next_account_info(accounts_iter)?;
        let clock = Self::get_clock_from_next_account(accounts_iter)?;

        if !rent_collector_account_info.is_signer {
            return Err(ProgramError::MissingRequiredSignature);
        }

        let multisig_op = MultisigOp::unpack(&multisig_op_account_info.data.borrow())?;
        let mut program_config =
            ProgramConfig::unpack(&program_config_account_info.data.borrow_mut())?;

        let expected_params = MultisigOpParams::CreateWallet {
            wallet_guid_hash: *wallet_guid_hash,
            program_config_address: *program_config_account_info.key,
            config_update: config_update.clone(),
        };

        if multisig_op.approved(&expected_params, &clock)? {
            program_config.add_wallet_config(wallet_guid_hash, config_update)?;
            ProgramConfig::pack(
                program_config,
                &mut program_config_account_info.data.borrow_mut(),
            )?;
        }

        Self::collect_remaining_balance(&multisig_op_account_info, &rent_collector_account_info)?;

        Ok(())
    }

    fn handle_init_wallet_config_update(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        wallet_guid_hash: &[u8; 32],
        config_update: &WalletConfigUpdate,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let multisig_op_account_info = Self::next_program_account_info(accounts_iter, program_id)?;
        let program_config_account_info =
            Self::next_program_account_info(accounts_iter, program_id)?;
        let initiator_account_info = next_account_info(accounts_iter)?;
        let clock = Self::get_clock_from_next_account(accounts_iter)?;

        let program_config = ProgramConfig::unpack(&program_config_account_info.data.borrow())?;
        program_config.validate_config_initiator(initiator_account_info)?;
        program_config.validate_wallet_config_update(wallet_guid_hash, config_update)?;

        let mut multisig_op =
            MultisigOp::unpack_unchecked(&multisig_op_account_info.data.borrow())?;
        multisig_op.init(
            program_config.get_config_approvers_keys(),
            program_config.approvals_required_for_config,
            clock.unix_timestamp,
            Self::calculate_expires(
                clock.unix_timestamp,
                program_config.approval_timeout_for_config,
            )?,
            MultisigOpParams::UpdateWalletConfig {
                program_config_address: *program_config_account_info.key,
                wallet_guid_hash: *wallet_guid_hash,
                config_update: config_update.clone(),
            },
        )?;
        MultisigOp::pack(multisig_op, &mut multisig_op_account_info.data.borrow_mut())?;

        Ok(())
    }

    fn handle_finalize_wallet_config_update(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        wallet_guid_hash: &[u8; 32],
        config_update: &WalletConfigUpdate,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let multisig_op_account_info = Self::next_program_account_info(accounts_iter, program_id)?;
        let program_config_account_info =
            Self::next_program_account_info(accounts_iter, program_id)?;
        let rent_collector_account_info = next_account_info(accounts_iter)?;
        let clock = Self::get_clock_from_next_account(accounts_iter)?;

        if !rent_collector_account_info.is_signer {
            return Err(ProgramError::MissingRequiredSignature);
        }

        let multisig_op = MultisigOp::unpack(&multisig_op_account_info.data.borrow())?;

        let expected_params = MultisigOpParams::UpdateWalletConfig {
            wallet_guid_hash: *wallet_guid_hash,
            program_config_address: *program_config_account_info.key,
            config_update: config_update.clone(),
        };
        if multisig_op.approved(&expected_params, &clock)? {
            let mut program_config =
                ProgramConfig::unpack(&program_config_account_info.data.borrow())?;
            program_config.update_wallet_config(wallet_guid_hash, config_update)?;
            ProgramConfig::pack(
                program_config,
                &mut program_config_account_info.data.borrow_mut(),
            )?;
        }

        Self::collect_remaining_balance(&multisig_op_account_info, &rent_collector_account_info)?;

        Ok(())
    }

    fn handle_init_transfer(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        wallet_guid_hash: &[u8; 32],
        amount: u64,
        destination_name_hash: &[u8; 32],
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let multisig_op_account_info = Self::next_program_account_info(accounts_iter, program_id)?;
        let program_config_account_info =
            Self::next_program_account_info(accounts_iter, program_id)?;
        let source_account = next_account_info(accounts_iter)?;
        let destination_account = next_account_info(accounts_iter)?;
        let initiator_account_info = next_account_info(accounts_iter)?;
        let clock = Self::get_clock_from_next_account(accounts_iter)?;
        let token_mint = next_account_info(accounts_iter)?;
        let destination_token_account = next_account_info(accounts_iter)?;

        let program_config = ProgramConfig::unpack(&program_config_account_info.data.borrow())?;
        let wallet_config = program_config.get_wallet_config(wallet_guid_hash)?;

        if !program_config.destination_allowed(
            wallet_config,
            destination_account.key,
            destination_name_hash,
        )? {
            msg!("Destination account is not whitelisted");
            return Err(WalletError::DestinationNotAllowed.into());
        }

        program_config.validate_transfer_initiator(wallet_config, initiator_account_info)?;

        if *token_mint.key != Pubkey::default()
            && *destination_token_account.owner == Pubkey::default()
        {
            // we need to create the destination token account (if it had been created already
            // it would be owned by the Token program).
            // frst check if the source account has sufficient funds to create it
            let rent = Rent::get()?;
            if rent.is_exempt(source_account.lamports(), Account::LEN) {
                let (source_account_pda, bump_seed) =
                    Pubkey::find_program_address(&[&wallet_guid_hash[..]], program_id);
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
                    &[&[&wallet_guid_hash[..], &[bump_seed]]],
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

        let mut multisig_op =
            MultisigOp::unpack_unchecked(&multisig_op_account_info.data.borrow())?;
        multisig_op.init(
            program_config.get_transfer_approvers_keys(wallet_config),
            wallet_config.approvals_required_for_transfer,
            clock.unix_timestamp,
            Self::calculate_expires(
                clock.unix_timestamp,
                wallet_config.approval_timeout_for_transfer,
            )?,
            MultisigOpParams::Transfer {
                program_config_address: *program_config_account_info.key,
                wallet_guid_hash: *wallet_guid_hash,
                destination: *destination_account.key,
                amount,
                token_mint: *token_mint.key,
            },
        )?;
        MultisigOp::pack(multisig_op, &mut multisig_op_account_info.data.borrow_mut())?;
        Ok(())
    }

    fn handle_finalize_transfer(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        wallet_guid_hash: [u8; 32],
        amount: u64,
        token_mint: Pubkey,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let multisig_op_account_info = Self::next_program_account_info(accounts_iter, program_id)?;
        let program_config_account_info =
            Self::next_program_account_info(accounts_iter, program_id)?;
        let source_account = next_account_info(accounts_iter)?;
        let destination_account = next_account_info(accounts_iter)?;
        let system_program_account = next_account_info(accounts_iter)?;
        let rent_collector_account_info = next_account_info(accounts_iter)?;
        let clock = Self::get_clock_from_next_account(accounts_iter)?;

        if !rent_collector_account_info.is_signer {
            return Err(ProgramError::MissingRequiredSignature);
        }

        let is_spl = token_mint.to_bytes() != [0; 32];

        if system_program_account.key != &system_program::id() {
            return Err(ProgramError::InvalidArgument);
        }

        let multisig_op = MultisigOp::unpack(&multisig_op_account_info.data.borrow())?;

        let expected_params = MultisigOpParams::Transfer {
            program_config_address: *program_config_account_info.key,
            wallet_guid_hash,
            destination: *destination_account.key,
            amount,
            token_mint,
        };

        if multisig_op.approved(&expected_params, &clock)? {
            let bump_seed = Self::validate_balance_account_and_get_seed(
                source_account,
                wallet_guid_hash,
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
                    &[&[&wallet_guid_hash[..], &[bump_seed]]],
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
                    wallet_guid_hash,
                    bump_seed,
                    system_program_account.clone(),
                    destination_account.clone(),
                    amount,
                )?;
            }
        }

        Self::collect_remaining_balance(&multisig_op_account_info, &rent_collector_account_info)?;

        Ok(())
    }

    fn handle_init_wrap_unwrap(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        wallet_guid_hash: [u8; 32],
        amount: u64,
        direction: WrapDirection,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let multisig_op_account = Self::next_program_account_info(accounts_iter, program_id)?;
        let program_config_account = Self::next_program_account_info(accounts_iter, program_id)?;
        let balance_account = next_account_info(accounts_iter)?;
        let wrapped_sol_account = next_account_info(accounts_iter)?;
        let native_mint_account = next_account_info(accounts_iter)?;
        if *native_mint_account.key != spl_token::native_mint::id() {
            msg!("Invalid native mint account set");
            return Err(ProgramError::InvalidAccountData);
        }

        let initiator_account = next_account_info(accounts_iter)?;
        let clock = Self::get_clock_from_next_account(accounts_iter)?;

        let program_config = ProgramConfig::unpack(&program_config_account.data.borrow())?;
        let wallet_config = program_config.get_wallet_config(&wallet_guid_hash)?;

        program_config.validate_transfer_initiator(wallet_config, initiator_account)?;

        if direction == WrapDirection::WRAP && *wrapped_sol_account.owner == Pubkey::default() {
            // we need to create the wrapped SOL account (if it had been created already
            // it would be owned by the Token program). Since this is an attempt to wrap
            // SOL, it stands to reason they have some SOL in their account, so we assume
            // they have enough to create this account (if they don't, it will just fail)
            let bump_seed = Self::validate_balance_account_and_get_seed(
                balance_account,
                wallet_guid_hash,
                program_id,
            )?;
            invoke_signed(
                &Instruction {
                    program_id: spl_associated_token_account::id(),
                    accounts: vec![
                        AccountMeta::new(*balance_account.key, true),
                        AccountMeta::new(*wrapped_sol_account.key, false),
                        AccountMeta::new_readonly(*balance_account.key, false),
                        AccountMeta::new_readonly(*native_mint_account.key, false),
                        AccountMeta::new_readonly(solana_program::system_program::id(), false),
                        AccountMeta::new_readonly(spl_token::id(), false),
                        AccountMeta::new_readonly(sysvar::rent::id(), false),
                    ],
                    data: vec![],
                },
                accounts,
                &[&[&wallet_guid_hash[..], &[bump_seed]]],
            )?;
        }

        let mut multisig_op = MultisigOp::unpack_unchecked(&multisig_op_account.data.borrow())?;
        multisig_op.init(
            program_config.get_transfer_approvers_keys(wallet_config),
            wallet_config.approvals_required_for_transfer,
            clock.unix_timestamp,
            Self::calculate_expires(
                clock.unix_timestamp,
                wallet_config.approval_timeout_for_transfer,
            )?,
            MultisigOpParams::Wrap {
                program_config_address: *program_config_account.key,
                wallet_guid_hash,
                amount,
                direction,
            },
        )?;
        MultisigOp::pack(multisig_op, &mut multisig_op_account.data.borrow_mut())?;
        Ok(())
    }

    fn handle_finalize_wrap_unwrap(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        wallet_guid_hash: [u8; 32],
        amount: u64,
        direction: WrapDirection,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let multisig_op_account = Self::next_program_account_info(accounts_iter, program_id)?;
        let program_config_account = Self::next_program_account_info(accounts_iter, program_id)?;
        let balance_account = next_account_info(accounts_iter)?;
        let system_program_account = next_account_info(accounts_iter)?;
        let rent_collector_account = next_account_info(accounts_iter)?;
        let clock = Self::get_clock_from_next_account(accounts_iter)?;
        let wrapped_sol_account = next_account_info(accounts_iter)?;

        if !rent_collector_account.is_signer {
            return Err(ProgramError::MissingRequiredSignature);
        }

        if system_program_account.key != &system_program::id() {
            return Err(ProgramError::InvalidArgument);
        }

        let multisig_op = MultisigOp::unpack(&multisig_op_account.data.borrow())?;

        let expected_params = MultisigOpParams::Wrap {
            program_config_address: *program_config_account.key,
            wallet_guid_hash,
            amount,
            direction,
        };

        if multisig_op.approved(&expected_params, &clock)? {
            let bump_seed = Self::validate_balance_account_and_get_seed(
                balance_account,
                wallet_guid_hash,
                program_id,
            )?;

            let wrapped_sol_account_key =
                get_associated_token_address(balance_account.key, &spl_token::native_mint::id());
            if *wrapped_sol_account.key != wrapped_sol_account_key {
                return Err(WalletError::InvalidSourceTokenAccount.into());
            }

            if direction == WrapDirection::WRAP {
                Self::transfer_sol_checked(
                    balance_account.clone(),
                    wallet_guid_hash,
                    bump_seed,
                    system_program_account.clone(),
                    wrapped_sol_account.clone(),
                    amount,
                )?;
            } else {
                let wrapped_sol_account_data =
                    SPLAccount::unpack(&wrapped_sol_account.data.borrow())?;
                if wrapped_sol_account_data.amount < amount {
                    msg!(
                        "Wrapped SOL account only has {} lamports of {} requested",
                        wrapped_sol_account_data.amount,
                        amount
                    );
                    return Err(WalletError::InsufficientBalance.into());
                }

                // the only way to transfer lamports out of a token acocunt is to close it, so we first
                // close it and then transfer back whatever is remaining
                let remaining = wrapped_sol_account
                    .lamports()
                    .checked_sub(amount)
                    .ok_or(WalletError::AmountOverflow)?;

                invoke_signed(
                    &spl_token::instruction::close_account(
                        &spl_token::id(),
                        &wrapped_sol_account.key,
                        &balance_account.key,
                        &balance_account.key,
                        &[],
                    )?,
                    &[wrapped_sol_account.clone(), balance_account.clone()],
                    &[&[&wallet_guid_hash[..], &[bump_seed]]],
                )?;

                Self::transfer_sol_checked(
                    balance_account.clone(),
                    wallet_guid_hash,
                    bump_seed,
                    system_program_account.clone(),
                    wrapped_sol_account.clone(),
                    remaining,
                )?;
            }

            invoke(
                &spl_token::instruction::sync_native(&spl_token::id(), &wrapped_sol_account_key)?,
                &[wrapped_sol_account.clone()],
            )?;
        }

        Self::collect_remaining_balance(&multisig_op_account, &rent_collector_account)?;

        Ok(())
    }

    fn handle_approval_disposition(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        disposition: ApprovalDisposition,
        params_hash: Hash,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let multisig_op_account_info = Self::next_program_account_info(accounts_iter, program_id)?;
        let signer_account_info = next_account_info(accounts_iter)?;
        let clock = Self::get_clock_from_next_account(accounts_iter)?;

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

    fn collect_remaining_balance(from: &AccountInfo, to: &AccountInfo) -> ProgramResult {
        // this moves the lamports back to the fee payer.
        **to.lamports.borrow_mut() = to
            .lamports()
            .checked_add(from.lamports())
            .ok_or(WalletError::AmountOverflow)?;
        **from.lamports.borrow_mut() = 0;
        *from.data.borrow_mut() = &mut [];

        Ok(())
    }

    fn next_program_account_info<'a, 'b, I: Iterator<Item = &'a AccountInfo<'b>>>(
        iter: &mut I,
        program_id: &Pubkey,
    ) -> Result<I::Item, ProgramError> {
        let account_info = next_account_info(iter)?;
        if account_info.owner != program_id {
            msg!("Account does not belong to the program");
            return Err(ProgramError::IncorrectProgramId);
        }
        Ok(account_info)
    }

    fn get_clock_from_next_account(iter: &mut Iter<AccountInfo>) -> Result<Clock, ProgramError> {
        let account_info = next_account_info(iter)?;
        if !is_sysvar_id(account_info.key) {
            msg!("Account is not a sysvar");
            return Err(ProgramError::InvalidArgument);
        }
        Clock::from_account_info(&account_info)
    }

    fn calculate_expires(start: i64, duration: Duration) -> Result<i64, ProgramError> {
        let expires_at = start.checked_add(duration.as_secs() as i64);
        if expires_at == None {
            return Err(ProgramError::InvalidArgument);
        }
        Ok(expires_at.unwrap())
    }

    fn transfer_sol_checked<'a>(
        balance_account: AccountInfo<'a>,
        wallet_guid_hash: [u8; 32],
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
            &[&[&wallet_guid_hash[..], &[bump_seed]]],
        )
    }

    fn validate_balance_account_and_get_seed(
        balance_account: &AccountInfo,
        wallet_guid_hash: [u8; 32],
        program_id: &Pubkey,
    ) -> Result<u8, ProgramError> {
        let (account_pda, bump_seed) =
            Pubkey::find_program_address(&[&wallet_guid_hash[..]], program_id);
        if &account_pda != balance_account.key {
            Err(WalletError::InvalidSourceAccount.into())
        } else {
            Ok(bump_seed)
        }
    }
}
