use crate::error::WalletError;
use crate::handlers::utils::{
    create_associated_token_account_instruction, finalize_multisig_op, get_clock_from_next_account,
    next_program_account_info, start_multisig_transfer_op, transfer_sol_checked,
    validate_balance_account_and_get_seed,
};
use crate::model::address_book::AddressBookEntryNameHash;
use crate::model::balance_account::BalanceAccountGuidHash;
use crate::model::multisig_op::MultisigOpParams;
use crate::model::wallet::Wallet;
use solana_program::account_info::{next_account_info, AccountInfo};
use solana_program::entrypoint::ProgramResult;
use solana_program::msg;
use solana_program::program::{invoke, invoke_signed};
use solana_program::program_error::ProgramError;
use solana_program::program_pack::Pack;
use solana_program::pubkey::Pubkey;
use solana_program::rent::Rent;
use solana_program::system_program;
use solana_program::sysvar::Sysvar;
use spl_associated_token_account::get_associated_token_address;
use spl_token::id as SPL_TOKEN_ID;
use spl_token::instruction as spl_instruction;
use spl_token::state::Account as SPLAccount;

pub fn init(
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
    let fee_payer_account = next_account_info(accounts_iter)?;

    let wallet = Wallet::unpack(&wallet_account_info.data.borrow())?;
    let balance_account = wallet.get_balance_account(account_guid_hash)?;

    if !wallet.destination_allowed(
        &balance_account,
        destination_account.key,
        destination_name_hash,
    )? {
        msg!("Destination account is not whitelisted");
        return Err(WalletError::DestinationNotAllowed.into());
    }

    wallet.validate_transfer_initiator(&balance_account, initiator_account_info)?;

    if *token_mint.key != Pubkey::default() && *destination_token_account.owner == Pubkey::default()
    {
        // We need to create the associated token "destination" account. If it had
        // been created already, it would be owned by the associated token program.

        // frst check if the source account has sufficient funds to create it
        let rent = Rent::get()?;
        if rent.is_exempt(source_account.lamports(), SPLAccount::LEN) {
            match validate_balance_account_and_get_seed(
                source_account,
                account_guid_hash,
                program_id,
            ) {
                Ok(bump_seed) => {
                    // pay for associated token account with source account
                    invoke_signed(
                        &create_associated_token_account_instruction(
                            source_account,
                            destination_token_account,
                            destination_account,
                            token_mint,
                        ),
                        accounts,
                        &[&[&account_guid_hash.to_bytes(), &[bump_seed]]],
                    )?;
                }
                Err(error) => {
                    if error == WalletError::InvalidPDA.into() {
                        msg!("could not find BalanceAccount PDA for source GUID hash");
                        return Err(WalletError::InvalidSourceAccount.into());
                    } else {
                        msg!("unhandled error validating source BalanceAccount GUID hash");
                        return Err(ProgramError::InvalidArgument);
                    }
                }
            }
        } else {
            // pay for associated token account with fee-payer account.
            invoke(
                &create_associated_token_account_instruction(
                    fee_payer_account,
                    destination_token_account,
                    destination_account,
                    token_mint,
                ),
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

pub fn finalize(
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
        return Err(WalletError::AccountNotRecognized.into());
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

                transfer_sol_checked(
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
