use crate::error::WalletError;
use crate::model::balance_account::{BalanceAccount, BalanceAccountGuidHash};
use crate::model::multisig_op::{MultisigOp, MultisigOpParams};
use crate::model::wallet::Wallet;
use solana_program::account_info::{next_account_info, AccountInfo};
use solana_program::clock::Clock;
use solana_program::entrypoint::ProgramResult;
use solana_program::msg;
use solana_program::program_error::ProgramError;
use solana_program::program_pack::Pack;
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
        return Err(WalletError::AccountNotRecognized.into());
    }
    Clock::from_account_info(&account_info)
}

pub fn calculate_expires(start: i64, duration: Duration) -> Result<i64, ProgramError> {
    let expires_at = start.checked_add(duration.as_secs() as i64);
    if expires_at == None {
        msg!("Invalid expires_at");
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

pub fn start_multisig_transfer_op(
    multisig_op_account_info: &AccountInfo,
    wallet: &Wallet,
    balance_account: &BalanceAccount,
    clock: Clock,
    params: MultisigOpParams,
) -> ProgramResult {
    let mut multisig_op = MultisigOp::unpack_unchecked(&multisig_op_account_info.data.borrow())?;

    multisig_op.init(
        wallet.get_transfer_approvers_keys(balance_account),
        balance_account.approvals_required_for_transfer,
        clock.unix_timestamp,
        calculate_expires(
            clock.unix_timestamp,
            balance_account.approval_timeout_for_transfer,
        )?,
        params,
    )?;
    MultisigOp::pack(multisig_op, &mut multisig_op_account_info.data.borrow_mut())?;

    Ok(())
}

pub fn start_multisig_config_op(
    multisig_op_account_info: &AccountInfo,
    wallet: &Wallet,
    clock: Clock,
    params: MultisigOpParams,
) -> ProgramResult {
    let mut multisig_op = MultisigOp::unpack_unchecked(&multisig_op_account_info.data.borrow())?;

    multisig_op.init(
        wallet.get_config_approvers_keys(),
        wallet.approvals_required_for_config,
        clock.unix_timestamp,
        calculate_expires(clock.unix_timestamp, wallet.approval_timeout_for_config)?,
        params,
    )?;
    MultisigOp::pack(multisig_op, &mut multisig_op_account_info.data.borrow_mut())?;

    Ok(())
}

pub fn finalize_multisig_op<F>(
    multisig_op_account_info: &AccountInfo,
    account_to_return_rent_to: &AccountInfo,
    clock: Clock,
    expected_params: MultisigOpParams,
    mut on_op_approved: F,
) -> ProgramResult
where
    F: FnMut() -> ProgramResult,
{
    if !account_to_return_rent_to.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }

    let multisig_op = MultisigOp::unpack(&multisig_op_account_info.data.borrow())?;

    if multisig_op.approved(&expected_params, &clock)? {
        on_op_approved()?
    }

    collect_remaining_balance(&multisig_op_account_info, &account_to_return_rent_to)?;

    Ok(())
}
