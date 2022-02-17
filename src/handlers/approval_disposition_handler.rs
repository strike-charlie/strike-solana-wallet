use crate::error::WalletError;
use crate::handlers::utils::{get_clock_from_next_account, next_program_account_info};
use crate::model::multisig_op::{ApprovalDisposition, MultisigOp};
use solana_program::account_info::{next_account_info, AccountInfo};
use solana_program::entrypoint::ProgramResult;
use solana_program::hash::Hash;
use solana_program::program_pack::Pack;
use solana_program::pubkey::Pubkey;

pub fn handle(
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
