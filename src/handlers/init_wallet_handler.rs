use crate::handlers::utils::next_program_account_info;
use crate::instruction::WalletUpdate;
use crate::model::signer::Signer;
use crate::model::wallet::Wallet;
use solana_program::account_info::{next_account_info, AccountInfo};
use solana_program::entrypoint::ProgramResult;
use solana_program::program_error::ProgramError;
use solana_program::program_pack::{IsInitialized, Pack};
use solana_program::pubkey::Pubkey;

pub fn handle(
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
