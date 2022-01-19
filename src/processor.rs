use std::time::Duration;

use solana_program::account_info::{next_account_info, AccountInfo};
use solana_program::entrypoint::ProgramResult;
use solana_program::msg;
use solana_program::program::invoke_signed;
use solana_program::program_error::ProgramError;
use solana_program::program_pack::{IsInitialized, Pack};
use solana_program::pubkey::Pubkey;
use solana_program::system_instruction;
use solana_program::system_program;
use solana_program::sysvar::{Sysvar, is_sysvar_id};
use spl_associated_token_account::get_associated_token_address;
use spl_token::id as SPL_TOKEN_ID;
use spl_token::instruction as spl_instruction;
use spl_token::state::Account as SPLAccount;

use crate::error::WalletError;
use crate::instruction::{ProgramConfigUpdate, ProgramInstruction, WalletConfigUpdate};
use crate::model::multisig_op::{ApprovalDisposition, MultisigOp, MultisigOpParams};
use crate::model::program_config::ProgramConfig;
use crate::model::wallet_config::WalletConfig;
use solana_program::clock::Clock;

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
                wallet_guid_hash,
                &config_update,
            ),

            ProgramInstruction::InitWalletConfigUpdate {
                wallet_guid_hash,
                config_update,
            } => Self::handle_init_wallet_config_update(
                program_id,
                accounts,
                wallet_guid_hash,
                &config_update,
            ),

            ProgramInstruction::FinalizeWalletConfigUpdate {
                wallet_guid_hash,
                config_update,
            } => Self::handle_finalize_wallet_config_update(
                program_id,
                accounts,
                wallet_guid_hash,
                &config_update,
            ),

            ProgramInstruction::InitTransfer {
                amount,
                destination_name_hash,
                token_mint,
            } => Self::handle_init_transfer(
                program_id,
                &accounts,
                amount,
                &destination_name_hash,
                token_mint,
            ),

            ProgramInstruction::FinalizeTransfer { amount, token_mint } => {
                Self::handle_finalize_transfer(program_id, &accounts, amount, token_mint)
            }

            ProgramInstruction::SetApprovalDisposition { disposition } => {
                Self::handle_approval_disposition(program_id, &accounts, disposition)
            }
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

        ProgramConfig::validate_initial_settings(config_update)?;

        let mut program_config =
            ProgramConfig::unpack_unchecked(&program_config_account_info.data.borrow())?;

        if program_config.is_initialized() {
            return Err(ProgramError::AccountAlreadyInitialized);
        }

        program_config.is_initialized = true;
        program_config.update(config_update)?;
        program_config.assistant = *assistant_account_info.key;
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
        let clock_sysvar_info = Self::next_sysvar_account_info(accounts_iter)?;
        let clock = Clock::from_account_info(&clock_sysvar_info)?;

        let program_config = ProgramConfig::unpack(&program_config_account_info.data.borrow())?;
        program_config.validate_initiator(initiator_account_info, &program_config.assistant)?;
        program_config.validate_update(config_update)?;

        let mut multisig_op =
            MultisigOp::unpack_unchecked(&multisig_op_account_info.data.borrow())?;

        multisig_op.init(
            program_config.config_approvers,
            program_config.approvals_required_for_config,
            clock.unix_timestamp,
            Self::calculate_expires(
                clock.unix_timestamp,
                program_config.approval_timeout_for_config
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
        let multisig_op_account_info =
            Self::next_program_account_info(accounts_iter, program_id)?;
        let program_config_account_info =
            Self::next_program_account_info(accounts_iter, program_id)?;
        let account_to_return_rent_to = next_account_info(accounts_iter)?;
        let clock_sysvar_info = Self::next_sysvar_account_info(accounts_iter)?;
        let clock = Clock::from_account_info(&clock_sysvar_info)?;

        if !account_to_return_rent_to.is_signer {
            return Err(ProgramError::MissingRequiredSignature);
        }

        let multisig_op =
            MultisigOp::unpack(&multisig_op_account_info.data.borrow())?;

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
        let clock_sysvar_info = Self::next_sysvar_account_info(accounts_iter)?;
        let clock = Clock::from_account_info(&clock_sysvar_info)?;

        let mut multisig_op =
            MultisigOp::unpack_unchecked(&multisig_op_account_info.data.borrow())?;
        let program_config = ProgramConfig::unpack(&program_config_account_info.data.borrow())?;
        program_config.validate_initiator(initiator_account_info, &program_config.assistant)?;

        WalletConfig::validate_initial_settings(config_update)?;

        multisig_op.init(
            program_config.config_approvers,
            program_config.approvals_required_for_config,
            clock.unix_timestamp,
            Self::calculate_expires(clock.unix_timestamp, program_config.approval_timeout_for_config)?,
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
        wallet_guid_hash: [u8; 32],
        config_update: &WalletConfigUpdate,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let multisig_op_account_info = Self::next_program_account_info(accounts_iter, program_id)?;
        let program_config_account_info =
            Self::next_program_account_info(accounts_iter, program_id)?;
        let wallet_config_account_info =
            Self::next_program_account_info(accounts_iter, program_id)?;
        let rent_collector_account_info = next_account_info(accounts_iter)?;
        let clock_sysvar_info = Self::next_sysvar_account_info(accounts_iter)?;
        let clock = Clock::from_account_info(&clock_sysvar_info)?;

        if !rent_collector_account_info.is_signer {
            return Err(ProgramError::MissingRequiredSignature);
        }

        let multisig_op = MultisigOp::unpack(&multisig_op_account_info.data.borrow())?;

        let expected_params = MultisigOpParams::CreateWallet {
            wallet_guid_hash,
            program_config_address: *program_config_account_info.key,
            config_update: config_update.clone(),
        };

        if multisig_op.approved(&expected_params, &clock)? {
            let mut wallet_config =
                WalletConfig::unpack_unchecked(&wallet_config_account_info.data.borrow_mut())?;
            if wallet_config.is_initialized() {
                return Err(ProgramError::AccountAlreadyInitialized);
            }
            wallet_config.is_initialized = true;
            wallet_config.wallet_guid_hash = wallet_guid_hash;
            wallet_config.program_config_address = *program_config_account_info.key;
            wallet_config.update(config_update)?;
            WalletConfig::pack(
                wallet_config,
                &mut wallet_config_account_info.data.borrow_mut(),
            )?;
        }

        Self::collect_remaining_balance(&multisig_op_account_info, &rent_collector_account_info)?;

        Ok(())
    }

    fn handle_init_wallet_config_update(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        wallet_guid_hash: [u8; 32],
        config_update: &WalletConfigUpdate,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let multisig_op_account_info = Self::next_program_account_info(accounts_iter, program_id)?;
        let wallet_config_account_info =
            Self::next_program_account_info(accounts_iter, program_id)?;
        let program_config_account_info =
            Self::next_program_account_info(accounts_iter, program_id)?;
        let initiator_account_info = next_account_info(accounts_iter)?;
        let clock_sysvar_info = Self::next_sysvar_account_info(accounts_iter)?;
        let clock = Clock::from_account_info(&clock_sysvar_info)?;

        let wallet_config = WalletConfig::unpack(&wallet_config_account_info.data.borrow())?;

        if wallet_config.program_config_address != *program_config_account_info.key {
            return Err(WalletError::InvalidConfigAccount.into());
        }

        wallet_config.validate_update(config_update)?;

        let program_config = ProgramConfig::unpack(&program_config_account_info.data.borrow())?;
        wallet_config.validate_initiator(initiator_account_info, &program_config.assistant)?;
        let mut multisig_op =
            MultisigOp::unpack_unchecked(&multisig_op_account_info.data.borrow())?;
        multisig_op.init(
            program_config.config_approvers,
            program_config.approvals_required_for_config,
            clock.unix_timestamp,
            Self::calculate_expires(clock.unix_timestamp, program_config.approval_timeout_for_config)?,
            MultisigOpParams::UpdateWalletConfig {
                wallet_guid_hash,
                wallet_config_address: *wallet_config_account_info.key,
                config_update: config_update.clone(),
            },
        )?;
        MultisigOp::pack(multisig_op, &mut multisig_op_account_info.data.borrow_mut())?;

        Ok(())
    }

    fn handle_finalize_wallet_config_update(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        wallet_guid_hash: [u8; 32],
        config_update: &WalletConfigUpdate,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let multisig_op_account_info = Self::next_program_account_info(accounts_iter, program_id)?;
        let wallet_config_account_info =
            Self::next_program_account_info(accounts_iter, program_id)?;
        let rent_collector_account_info = next_account_info(accounts_iter)?;
        let clock_sysvar_info = Self::next_sysvar_account_info(accounts_iter)?;
        let clock = Clock::from_account_info(&clock_sysvar_info)?;

        if !rent_collector_account_info.is_signer {
            return Err(ProgramError::MissingRequiredSignature);
        }

        let multisig_op = MultisigOp::unpack(&multisig_op_account_info.data.borrow())?;

        let expected_params = MultisigOpParams::UpdateWalletConfig {
            wallet_guid_hash,
            wallet_config_address: *wallet_config_account_info.key,
            config_update: config_update.clone(),
        };
        if multisig_op.approved(&expected_params, &clock)? {
            let mut wallet_config =
                WalletConfig::unpack(&wallet_config_account_info.data.borrow_mut())?;
            wallet_config.update(config_update)?;
            WalletConfig::pack(
                wallet_config,
                &mut wallet_config_account_info.data.borrow_mut(),
            )?;
        }

        Self::collect_remaining_balance(&multisig_op_account_info, &rent_collector_account_info)?;

        Ok(())
    }

    fn handle_init_transfer(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        amount: u64,
        destination_name_hash: &[u8; 32],
        token_mint: Pubkey,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let multisig_op_account_info = Self::next_program_account_info(accounts_iter, program_id)?;
        let wallet_config_account_info =
            Self::next_program_account_info(accounts_iter, program_id)?;
        let source_account = next_account_info(accounts_iter)?;
        let destination_account = next_account_info(accounts_iter)?;
        let program_account_info = Self::next_program_account_info(accounts_iter, program_id)?;
        let initiator_account_info = next_account_info(accounts_iter)?;
        let clock_sysvar_info = Self::next_sysvar_account_info(accounts_iter)?;
        let clock = Clock::from_account_info(&clock_sysvar_info)?;

        let wallet_config = WalletConfig::unpack(&wallet_config_account_info.data.borrow())?;

        if !wallet_config.destination_allowed(destination_account.key, destination_name_hash) {
            msg!("Destination account is not whitelisted");
            return Err(WalletError::DestinationNotAllowed.into());
        }

        if wallet_config.program_config_address != *program_account_info.key {
            msg!("Incorrect program account key provided");
            return Err(WalletError::InvalidConfigAccount.into());
        }

        let program_config = ProgramConfig::unpack(&program_account_info.data.borrow())?;
        wallet_config.validate_initiator(initiator_account_info, &program_config.assistant)?;

        let mut multisig_op =
            MultisigOp::unpack_unchecked(&multisig_op_account_info.data.borrow())?;
        multisig_op.init(
            wallet_config.approvers,
            wallet_config.approvals_required_for_transfer,
            clock.unix_timestamp,
            Self::calculate_expires(clock.unix_timestamp, wallet_config.approval_timeout_for_transfer)?,
            MultisigOpParams::Transfer {
                wallet_config_address: *wallet_config_account_info.key,
                source: *source_account.key,
                destination: *destination_account.key,
                amount,
                token_mint,
            },
        )?;
        MultisigOp::pack(multisig_op, &mut multisig_op_account_info.data.borrow_mut())?;
        Ok(())
    }

    fn handle_finalize_transfer(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        amount: u64,
        token_mint: Pubkey,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let multisig_op_account_info = Self::next_program_account_info(accounts_iter, program_id)?;
        let source_account = next_account_info(accounts_iter)?;
        let destination_account = next_account_info(accounts_iter)?;
        let wallet_config_account_info =
            Self::next_program_account_info(accounts_iter, program_id)?;
        let system_program_account = next_account_info(accounts_iter)?;
        let rent_collector_account_info = next_account_info(accounts_iter)?;
        let clock_sysvar_info = Self::next_sysvar_account_info(accounts_iter)?;
        let clock = Clock::from_account_info(&clock_sysvar_info)?;

        if !rent_collector_account_info.is_signer {
            return Err(ProgramError::MissingRequiredSignature);
        }

        let is_spl = token_mint.to_bytes() != [0; 32];

        if system_program_account.key != &system_program::id() {
            return Err(ProgramError::InvalidArgument);
        }

        let multisig_op = MultisigOp::unpack(&multisig_op_account_info.data.borrow())?;

        let expected_params = MultisigOpParams::Transfer {
            wallet_config_address: *wallet_config_account_info.key,
            source: *source_account.key,
            destination: *destination_account.key,
            amount,
            token_mint,
        };

        if multisig_op.approved(&expected_params, &clock)? {
            let (source_account_pda, bump_seed) = Pubkey::find_program_address(
                &[&wallet_config_account_info.key.to_bytes()],
                program_id,
            );
            if &source_account_pda != source_account.key {
                return Err(WalletError::InvalidSourceAccount.into());
            }
            if is_spl {
                let source_token_account = next_account_info(accounts_iter)?;
                let source_token_account_key =
                    get_associated_token_address(&source_account_pda, &token_mint);
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
                        &source_account_pda,
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
                    &[&[&wallet_config_account_info.key.to_bytes()[..], &[bump_seed]]],
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

                invoke_signed(
                    &system_instruction::transfer(
                        source_account.key,
                        destination_account.key,
                        amount,
                    ),
                    &[
                        source_account.clone(),
                        destination_account.clone(),
                        system_program_account.clone(),
                    ],
                    &[&[&wallet_config_account_info.key.to_bytes()[..], &[bump_seed]]],
                )?;
            }
        }

        Self::collect_remaining_balance(&multisig_op_account_info, &rent_collector_account_info)?;

        Ok(())
    }

    fn handle_approval_disposition(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        disposition: ApprovalDisposition,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let multisig_op_account_info = Self::next_program_account_info(accounts_iter, program_id)?;
        let signer_account_info = next_account_info(accounts_iter)?;
        let clock_sysvar_info = Self::next_sysvar_account_info(accounts_iter)?;
        let clock = Clock::from_account_info(&clock_sysvar_info)?;

        let mut multisig_op = MultisigOp::unpack(&multisig_op_account_info.data.borrow())?;
        multisig_op.validate_and_record_approval_disposition(&signer_account_info, disposition, &clock)?;
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

    fn next_sysvar_account_info<'a, 'b, I: Iterator<Item = &'a AccountInfo<'b>>>(
        iter: &mut I
    ) -> Result<I::Item, ProgramError> {
        let account_info = next_account_info(iter)?;
        if !is_sysvar_id(account_info.key) {
            msg!("Account does not have the correct key");
            return Err(ProgramError::InvalidArgument);
        }
        Ok(account_info)
    }

    fn calculate_expires(start: i64, duration: Duration) -> Result<i64, ProgramError> {
        let expires_at = start.checked_add(duration.as_secs() as i64);
        if expires_at == None {
            return Err(ProgramError::InvalidArgument);
        }
        Ok(expires_at.unwrap())
    }
}
