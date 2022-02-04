use crate::error::WalletError;
use crate::model::balance_account::BalanceAccountGuidHash;
use solana_program::account_info::{next_account_info, AccountInfo};
use solana_program::clock::Clock;
use solana_program::entrypoint::ProgramResult;
use solana_program::msg;
use solana_program::program_error::ProgramError;
use solana_program::pubkey::Pubkey;
use solana_program::sysvar::Sysvar;
use std::slice::Iter;
use std::time::Duration;

pub fn collect_remaining_balance(from: &AccountInfo, to: &AccountInfo) -> ProgramResult {
    // this moves the lamports back to the fee payer.
    **to.lamports.borrow_mut() = to
        .lamports()
        .checked_add(from.lamports())
        .ok_or(WalletError::AmountOverflow)?;
    **from.lamports.borrow_mut() = 0;
    *from.data.borrow_mut() = &mut [];

    Ok(())
}

pub fn next_program_account_info<'a, 'b, I: Iterator<Item = &'a AccountInfo<'b>>>(
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

pub fn get_clock_from_next_account(iter: &mut Iter<AccountInfo>) -> Result<Clock, ProgramError> {
    let account_info = next_account_info(iter)?;
    if solana_program::sysvar::clock::id() != *account_info.key {
        msg!("Invalid clock account");
        return Err(ProgramError::InvalidArgument);
    }
    Clock::from_account_info(&account_info)
}

pub fn calculate_expires(start: i64, duration: Duration) -> Result<i64, ProgramError> {
    let expires_at = start.checked_add(duration.as_secs() as i64);
    if expires_at == None {
        return Err(ProgramError::InvalidArgument);
    }
    Ok(expires_at.unwrap())
}

pub fn validate_balance_account_and_get_seed(
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
