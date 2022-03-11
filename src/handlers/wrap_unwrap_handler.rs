use crate::error::WalletError;
use crate::handlers::utils::{
    create_associated_token_account_instruction, finalize_multisig_op, get_clock_from_next_account,
    next_program_account_info, start_multisig_transfer_op, transfer_sol_checked,
    validate_balance_account_and_get_seed,
};
use crate::model::balance_account::BalanceAccountGuidHash;
use crate::model::multisig_op::{MultisigOpParams, WrapDirection};
use crate::model::wallet::Wallet;
use solana_program::account_info::{next_account_info, AccountInfo};
use solana_program::entrypoint::ProgramResult;
use solana_program::msg;
use solana_program::program::{invoke, invoke_signed};
use solana_program::program_error::ProgramError;
use solana_program::program_pack::Pack;
use solana_program::pubkey::Pubkey;
use solana_program::system_program;
use spl_associated_token_account::get_associated_token_address;
use spl_token::state::Account as SPLAccount;

pub fn init(
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

    wallet.validate_transfer_initiator(&balance_account, initiator_account)?;

    if direction == WrapDirection::WRAP && *wrapped_sol_account_info.owner == Pubkey::default() {
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
            &create_associated_token_account_instruction(
                balance_account_info,
                wrapped_sol_account_info,
                balance_account_info,
                native_mint_account_info,
            ),
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

pub fn finalize(
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
        return Err(WalletError::AccountNotRecognized.into());
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
                transfer_sol_checked(
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

                transfer_sol_checked(
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
            Ok(())
        },
    )
}
