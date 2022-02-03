use bitvec::macros::internal::funty::Fundamental;
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
use solana_program::sysvar::Sysvar;
use solana_program::{msg, sysvar};
use spl_associated_token_account::get_associated_token_address;
use spl_token::id as SPL_TOKEN_ID;
use spl_token::instruction as spl_instruction;
use spl_token::state::{Account as SPLAccount, Account};

use crate::error::WalletError;
use crate::handlers::utils::{
    calculate_expires, collect_remaining_balance, get_clock_from_next_account,
    next_program_account_info,
};
use crate::handlers::wallet_config_policy_update_handler;
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
            } => Self::handle_init_dapp_transaction(
                program_id,
                accounts,
                account_guid_hash,
                instructions,
            ),

            ProgramInstruction::FinalizeDAppTransaction {
                ref account_guid_hash,
                ref instructions,
            } => Self::handle_finalize_dapp_transaction(
                program_id,
                accounts,
                account_guid_hash,
                instructions,
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

        let mut multisig_op =
            MultisigOp::unpack_unchecked(&multisig_op_account_info.data.borrow())?;

        multisig_op.init(
            wallet.get_config_approvers_keys(),
            wallet.approvals_required_for_config,
            clock.unix_timestamp,
            calculate_expires(clock.unix_timestamp, wallet.approval_timeout_for_config)?,
            MultisigOpParams::UpdateWallet {
                wallet_address: *wallet_account_info.key,
                update: update.clone(),
            },
        )?;
        MultisigOp::pack(multisig_op, &mut multisig_op_account_info.data.borrow_mut())?;

        Ok(())
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

        if !account_to_return_rent_to.is_signer {
            return Err(ProgramError::MissingRequiredSignature);
        }

        let multisig_op = MultisigOp::unpack(&multisig_op_account_info.data.borrow())?;

        let expected_params = MultisigOpParams::UpdateWallet {
            wallet_address: *wallet_account_info.key,
            update: update.clone(),
        };

        if multisig_op.approved(&expected_params, &clock)? {
            let mut wallet = Wallet::unpack(&wallet_account_info.data.borrow_mut())?;
            wallet.update(update)?;
            Wallet::pack(wallet, &mut wallet_account_info.data.borrow_mut())?;
        }

        collect_remaining_balance(&multisig_op_account_info, &account_to_return_rent_to)?;

        Ok(())
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

        let mut multisig_op =
            MultisigOp::unpack_unchecked(&multisig_op_account_info.data.borrow())?;
        let wallet = Wallet::unpack(&wallet_account_info.data.borrow())?;
        wallet.validate_config_initiator(initiator_account_info)?;
        wallet.validate_add_balance_account(account_guid_hash, update)?;

        multisig_op.init(
            wallet.get_config_approvers_keys(),
            wallet.approvals_required_for_config,
            clock.unix_timestamp,
            calculate_expires(clock.unix_timestamp, wallet.approval_timeout_for_config)?,
            MultisigOpParams::CreateBalanceAccount {
                account_guid_hash: *account_guid_hash,
                wallet_address: *wallet_account_info.key,
                update: update.clone(),
            },
        )?;
        MultisigOp::pack(multisig_op, &mut multisig_op_account_info.data.borrow_mut())?;

        Ok(())
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

        if !rent_collector_account_info.is_signer {
            return Err(ProgramError::MissingRequiredSignature);
        }

        let multisig_op = MultisigOp::unpack(&multisig_op_account_info.data.borrow())?;
        let mut wallet = Wallet::unpack(&wallet_account_info.data.borrow_mut())?;

        let expected_params = MultisigOpParams::CreateBalanceAccount {
            account_guid_hash: *account_guid_hash,
            wallet_address: *wallet_account_info.key,
            update: update.clone(),
        };

        if multisig_op.approved(&expected_params, &clock)? {
            wallet.add_balance_account(account_guid_hash, update)?;
            Wallet::pack(wallet, &mut wallet_account_info.data.borrow_mut())?;
        }

        collect_remaining_balance(&multisig_op_account_info, &rent_collector_account_info)?;

        Ok(())
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

        let mut multisig_op =
            MultisigOp::unpack_unchecked(&multisig_op_account_info.data.borrow())?;
        multisig_op.init(
            wallet.get_config_approvers_keys(),
            wallet.approvals_required_for_config,
            clock.unix_timestamp,
            calculate_expires(clock.unix_timestamp, wallet.approval_timeout_for_config)?,
            MultisigOpParams::UpdateBalanceAccount {
                wallet_address: *wallet_account_info.key,
                account_guid_hash: *account_guid_hash,
                update: update.clone(),
            },
        )?;
        MultisigOp::pack(multisig_op, &mut multisig_op_account_info.data.borrow_mut())?;

        Ok(())
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

        if !rent_collector_account_info.is_signer {
            return Err(ProgramError::MissingRequiredSignature);
        }

        let multisig_op = MultisigOp::unpack(&multisig_op_account_info.data.borrow())?;

        let expected_params = MultisigOpParams::UpdateBalanceAccount {
            account_guid_hash: *account_guid_hash,
            wallet_address: *wallet_account_info.key,
            update: update.clone(),
        };
        if multisig_op.approved(&expected_params, &clock)? {
            let mut wallet = Wallet::unpack(&wallet_account_info.data.borrow())?;
            wallet.update_balance_account(account_guid_hash, update)?;
            Wallet::pack(wallet, &mut wallet_account_info.data.borrow_mut())?;
        }

        collect_remaining_balance(&multisig_op_account_info, &rent_collector_account_info)?;

        Ok(())
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

        let mut multisig_op =
            MultisigOp::unpack_unchecked(&multisig_op_account_info.data.borrow())?;
        multisig_op.init(
            wallet.get_transfer_approvers_keys(balance_account),
            balance_account.approvals_required_for_transfer,
            clock.unix_timestamp,
            calculate_expires(
                clock.unix_timestamp,
                balance_account.approval_timeout_for_transfer,
            )?,
            MultisigOpParams::Transfer {
                wallet_address: *wallet_account_info.key,
                account_guid_hash: *account_guid_hash,
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

        if !rent_collector_account_info.is_signer {
            return Err(ProgramError::MissingRequiredSignature);
        }

        let is_spl = token_mint.to_bytes() != [0; 32];

        if system_program_account.key != &system_program::id() {
            return Err(ProgramError::InvalidArgument);
        }

        let multisig_op = MultisigOp::unpack(&multisig_op_account_info.data.borrow())?;

        let expected_params = MultisigOpParams::Transfer {
            wallet_address: *wallet_account_info.key,
            account_guid_hash: *account_guid_hash,
            destination: *destination_account.key,
            amount,
            token_mint,
        };

        if multisig_op.approved(&expected_params, &clock)? {
            let bump_seed = Self::validate_balance_account_and_get_seed(
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
        }

        collect_remaining_balance(&multisig_op_account_info, &rent_collector_account_info)?;

        Ok(())
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
            let bump_seed = Self::validate_balance_account_and_get_seed(
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

        let mut multisig_op =
            MultisigOp::unpack_unchecked(&multisig_op_account_info.data.borrow())?;
        multisig_op.init(
            wallet.get_transfer_approvers_keys(balance_account),
            balance_account.approvals_required_for_transfer,
            clock.unix_timestamp,
            calculate_expires(
                clock.unix_timestamp,
                balance_account.approval_timeout_for_transfer,
            )?,
            MultisigOpParams::Wrap {
                wallet_address: *wallet_account_info.key,
                account_guid_hash: *account_guid_hash,
                amount,
                direction,
            },
        )?;
        MultisigOp::pack(multisig_op, &mut multisig_op_account_info.data.borrow_mut())?;
        Ok(())
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

        if !rent_collector_account_info.is_signer {
            return Err(ProgramError::MissingRequiredSignature);
        }

        if system_program_account_info.key != &system_program::id() {
            return Err(ProgramError::InvalidArgument);
        }

        let multisig_op = MultisigOp::unpack(&multisig_op_account_info.data.borrow())?;

        let expected_params = MultisigOpParams::Wrap {
            wallet_address: *wallet_account_info.key,
            account_guid_hash: *account_guid_hash,
            amount,
            direction,
        };

        if multisig_op.approved(&expected_params, &clock)? {
            let bump_seed = Self::validate_balance_account_and_get_seed(
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
                &spl_token::instruction::sync_native(&spl_token::id(), &wrapped_sol_account_key)?,
                &[wrapped_sol_account_info.clone()],
            )?;
        }

        collect_remaining_balance(&multisig_op_account_info, &rent_collector_account_info)?;

        Ok(())
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

        let mut multisig_op =
            MultisigOp::unpack_unchecked(&multisig_op_account_info.data.borrow())?;

        multisig_op.init(
            wallet.get_config_approvers_keys(),
            wallet.approvals_required_for_config,
            clock.unix_timestamp,
            calculate_expires(clock.unix_timestamp, wallet.approval_timeout_for_config)?,
            MultisigOpParams::UpdateSigner {
                wallet_address: *wallet_account_info.key,
                slot_update_type,
                slot_id,
                signer,
            },
        )?;
        MultisigOp::pack(multisig_op, &mut multisig_op_account_info.data.borrow_mut())?;

        Ok(())
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

        if !account_to_return_rent_to.is_signer {
            return Err(ProgramError::MissingRequiredSignature);
        }

        let multisig_op = MultisigOp::unpack(&multisig_op_account_info.data.borrow())?;

        let expected_params = MultisigOpParams::UpdateSigner {
            wallet_address: *wallet_account_info.key,
            slot_update_type,
            slot_id,
            signer,
        };

        if multisig_op.approved(&expected_params, &clock)? {
            let mut wallet = Wallet::unpack(&wallet_account_info.data.borrow_mut())?;
            match slot_update_type {
                SlotUpdateType::SetIfEmpty => wallet.add_signer((slot_id, signer))?,
                SlotUpdateType::Clear => wallet.remove_signer((slot_id, signer))?,
            }
            Wallet::pack(wallet, &mut wallet_account_info.data.borrow_mut())?;
        }

        collect_remaining_balance(&multisig_op_account_info, &account_to_return_rent_to)?;
        Self::collect_remaining_balance(&multisig_op_account_info, &account_to_return_rent_to)?;

        Ok(())
    }

    fn handle_init_dapp_transaction(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        account_guid_hash: &BalanceAccountGuidHash,
        instructions: Vec<Instruction>,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let multisig_op_account_info = Self::next_program_account_info(accounts_iter, program_id)?;
        let wallet_account_info = Self::next_program_account_info(accounts_iter, program_id)?;
        let initiator_account_info = next_account_info(accounts_iter)?;
        let clock = Self::get_clock_from_next_account(accounts_iter)?;

        let wallet = Wallet::unpack(&wallet_account_info.data.borrow())?;
        let balance_account = wallet.get_balance_account(account_guid_hash)?;

        wallet.validate_transfer_initiator(balance_account, initiator_account_info)?;

        let mut multisig_op =
            MultisigOp::unpack_unchecked(&multisig_op_account_info.data.borrow())?;
        multisig_op.init(
            wallet.get_transfer_approvers_keys(balance_account),
            1,
            clock.unix_timestamp,
            Self::calculate_expires(
                clock.unix_timestamp,
                balance_account.approval_timeout_for_transfer,
            )?,
            MultisigOpParams::DAppTransaction {
                wallet_address: *wallet_account_info.key,
                account_guid_hash: *account_guid_hash,
                instructions,
            },
        )?;
        MultisigOp::pack(multisig_op, &mut multisig_op_account_info.data.borrow_mut())?;
        Ok(())
    }

    fn account_balances(accounts: &[AccountInfo]) -> Vec<u64> {
        accounts.iter().map(|a| a.lamports()).collect()
    }

    fn spl_balances(accounts: &[AccountInfo]) -> Vec<SplBalance> {
        accounts
            .iter()
            .filter_map(|a| {
                if *a.owner == spl_token::id() {
                    if let Some(account_data) = SPLAccount::unpack(&a.data.borrow()).ok() {
                        Some(SplBalance {
                            account: *a.key,
                            token_mint: account_data.mint,
                            balance: account_data.amount,
                        })
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect()
    }

    fn balance_changes_from_simulation(
        starting_balances: Vec<u64>,
        starting_spl_balances: Vec<SplBalance>,
        accounts: &[AccountInfo],
    ) -> String {
        // compute and log just the changes to minimize compute budget spend
        let ending_balances = Self::account_balances(accounts);
        let mut balance_changes: Vec<(u8, char, u64)> = Vec::new();
        for i in 0..starting_balances.len() {
            if ending_balances[i] > starting_balances[i] {
                balance_changes.push((i as u8, '+', ending_balances[i] - starting_balances[i]));
            } else if ending_balances[i] < starting_balances[i] {
                balance_changes.push((i as u8, '-', starting_balances[i] - ending_balances[i]));
            }
        }
        let ending_spl_balances = Self::spl_balances(accounts);
        let mut spl_balance_changes: Vec<(u8, char, u64)> = Vec::new();
        for end in ending_spl_balances.into_iter() {
            let starting_balance = starting_spl_balances
                .iter()
                .find(|start| start.account == end.account && start.token_mint == end.token_mint)
                .map(|start| start.balance)
                .unwrap_or(0);
            if end.balance > starting_balance {
                spl_balance_changes.push((
                    accounts
                        .iter()
                        .position(|a| *a.key == end.account)
                        .unwrap()
                        .as_u8(),
                    '+',
                    end.balance - starting_balance,
                ));
            } else if starting_balance < end.balance {
                spl_balance_changes.push((
                    accounts
                        .iter()
                        .position(|a| *a.key == end.account)
                        .unwrap()
                        .as_u8(),
                    '-',
                    starting_balance - end.balance,
                ));
            }
        }
        format!(
            "Simulation balance changes: {:?} {:?}",
            balance_changes, spl_balance_changes
        )
    }

    fn handle_finalize_dapp_transaction(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        account_guid_hash: &BalanceAccountGuidHash,
        instructions: &Vec<Instruction>,
    ) -> ProgramResult {
        let accounts_iter = &mut accounts.iter();
        let multisig_op_account_info = Self::next_program_account_info(accounts_iter, program_id)?;
        let wallet_account_info = Self::next_program_account_info(accounts_iter, program_id)?;
        let balance_account = next_account_info(accounts_iter)?;
        let rent_collector_account_info = next_account_info(accounts_iter)?;
        let clock = Self::get_clock_from_next_account(accounts_iter)?;

        if !rent_collector_account_info.is_signer {
            return Err(ProgramError::MissingRequiredSignature);
        }

        let multisig_op = MultisigOp::unpack(&multisig_op_account_info.data.borrow())?;

        let expected_params = MultisigOpParams::DAppTransaction {
            wallet_address: *wallet_account_info.key,
            account_guid_hash: *account_guid_hash,
            instructions: instructions.clone(),
        };

        let is_approved = match multisig_op.approved(&expected_params, &clock) {
            Ok(a) => a,
            Err(e) => {
                msg!("Approval failed: {:?}", e);
                false
            }
        };

        let bump_seed = Self::validate_balance_account_and_get_seed(
            balance_account,
            account_guid_hash,
            program_id,
        )?;

        let starting_balances: Vec<u64> = if is_approved {
            Vec::new()
        } else {
            Self::account_balances(accounts)
        };

        let starting_spl_balances: Vec<SplBalance> = if is_approved {
            Vec::new()
        } else {
            Self::spl_balances(accounts)
        };

        for instruction in instructions.iter() {
            invoke_signed(
                &instruction,
                &accounts,
                &[&[&account_guid_hash.to_bytes(), &[bump_seed]]],
            )?;
        }

        if is_approved {
            Self::collect_remaining_balance(
                &multisig_op_account_info,
                &rent_collector_account_info,
            )?;

            Ok(())
        } else {
            msg!(&Self::balance_changes_from_simulation(
                starting_balances,
                starting_spl_balances,
                accounts,
            ));
            Err(WalletError::SimulationFinished.into())
        }
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

    fn validate_balance_account_and_get_seed(
        balance_account: &AccountInfo,
        account_guid_hash: &BalanceAccountGuidHash,
        program_id: &Pubkey,
    ) -> Result<u8, ProgramError> {
        let (account_pda, bump_seed) =
            Pubkey::find_program_address(&[&account_guid_hash.to_bytes()], program_id);
        if &account_pda != balance_account.key {
            Err(WalletError::InvalidSourceAccount.into())
        } else {
            Ok(bump_seed)
        }
    }
}

struct SplBalance {
    account: Pubkey,
    token_mint: Pubkey,
    balance: u64,
}
