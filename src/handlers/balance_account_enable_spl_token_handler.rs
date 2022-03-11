use crate::{
    error::WalletError,
    handlers::utils::{
        create_associated_token_account_instruction, finalize_multisig_op,
        get_clock_from_next_account, next_program_account_info, start_multisig_config_op,
        validate_balance_account_and_get_seed,
    },
    model::{
        balance_account::{BalanceAccount, BalanceAccountGuidHash},
        multisig_op::MultisigOpParams,
        wallet::Wallet,
    },
};
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    entrypoint::ProgramResult,
    msg,
    program::invoke_signed,
    program_error::ProgramError,
    program_pack::Pack,
    pubkey::Pubkey,
    rent::Rent,
    sysvar::Sysvar,
};
use spl_associated_token_account::get_associated_token_address;
use spl_token::state::Account as SPLAccount;

/// Maximum number of BalanceAccounts sent in `account_guid_hashes`
pub const MAX_BALANCE_ACCOUNT_GUID_HASHES: usize = 5;

pub fn init(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    payer_account_guid_hash: &BalanceAccountGuidHash,
    account_guid_hashes: &Vec<BalanceAccountGuidHash>,
) -> ProgramResult {
    let accounts_iter = &mut accounts.iter();

    let multisig_op_account_info = next_program_account_info(accounts_iter, program_id)?;
    let wallet_account_info = next_program_account_info(accounts_iter, program_id)?;
    let initiator_account_info = next_account_info(accounts_iter)?;
    let token_mint_account_info = next_account_info(accounts_iter)?;
    let clock = get_clock_from_next_account(accounts_iter)?;

    let wallet: Wallet = Wallet::unpack(&wallet_account_info.data.borrow())?;

    wallet.validate_config_initiator(initiator_account_info)?;

    if account_guid_hashes.len() == 0 {
        msg!("Missing BalanceAccountGuidHashes for which to enable the SPL token");
        return Err(ProgramError::NotEnoughAccountKeys);
    } else if account_guid_hashes.len() > MAX_BALANCE_ACCOUNT_GUID_HASHES {
        msg!("Too many BalanceAccountGuidHashes");
        return Err(ProgramError::InvalidArgument);
    }

    // ensure mint account owned by the spl-token program.
    if *token_mint_account_info.owner != spl_token::id() {
        msg!(
            "Token mint account {} not owned by spl-token program",
            token_mint_account_info.key,
        );
        return Err(WalletError::AccountNotRecognized.into());
    }

    // ensure balance accounts are associated with wallet
    wallet.validate_balance_account_guid_hash(payer_account_guid_hash)?;

    // validate each AT account with respect to its associated BalanceAccount.
    // Note that each AccountInfo following `clock` is an associated token
    // account, whose address is expected to be derived from the BalanceAccount
    // GUID at the corresponding index in `account_guid_hashes`.
    for account_guid_hash in account_guid_hashes.iter() {
        let associated_token_account_info = next_account_info(accounts_iter)?;

        wallet.validate_balance_account_guid_hash(account_guid_hash)?;

        // ensure AT account is not owned yet
        if *associated_token_account_info.owner != Pubkey::default() {
            msg!(
                "Associated token account {} already exists",
                associated_token_account_info.key
            );
            return Err(ProgramError::AccountAlreadyInitialized);
        }
        // ensure AT addr is correctly derived from BalanceAccount address
        let (balance_account_pda, _) = BalanceAccount::find_address(account_guid_hash, program_id);
        if get_associated_token_address(&balance_account_pda, &token_mint_account_info.key)
            != *associated_token_account_info.key
        {
            msg!(
                "BalanceAccount {} not authorized to associated token account {}",
                balance_account_pda,
                associated_token_account_info.key
            );
            return Err(WalletError::AccountNotRecognized.into());
        }
    }

    start_multisig_config_op(
        &multisig_op_account_info,
        &wallet,
        clock,
        MultisigOpParams::BalanceAccountEnableSplToken {
            wallet_address: *wallet_account_info.key,
            payer_account_guid_hash: *payer_account_guid_hash,
            account_guid_hashes: account_guid_hashes.clone(),
            token_mint: *token_mint_account_info.key,
        },
    )?;

    Wallet::pack(wallet, &mut wallet_account_info.data.borrow_mut())?;

    Ok(())
}

pub fn finalize(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    payer_account_guid_hash: &BalanceAccountGuidHash,
    account_guid_hashes: &Vec<BalanceAccountGuidHash>,
) -> ProgramResult {
    // BASE_ACCOUNTS_LEN is the Fixed number of accounts in instruction to
    // expect at a minimum. The number of additional accounts beyond this
    // varies, depending on the number of account GUID hashes.
    const BASE_ACCOUNTS_LEN: usize = 10;

    let accounts_iter = &mut accounts.iter();
    let multisig_op_account_info = next_program_account_info(accounts_iter, program_id)?;
    let wallet_account_info = next_program_account_info(accounts_iter, program_id)?;
    let rent_collector_account_info = next_account_info(accounts_iter)?;
    let token_mint_account_info = next_account_info(accounts_iter)?;
    let payer_balance_account_info = next_account_info(accounts_iter)?;
    let clock = get_clock_from_next_account(accounts_iter)?;
    let _system_account_info = next_account_info(accounts_iter)?;
    let _spl_associated_token_account_info = next_account_info(accounts_iter)?;
    let _spl_token_account_info = next_account_info(accounts_iter)?;
    let _rent_account_info = next_account_info(accounts_iter)?;

    let n_guid_hashes = account_guid_hashes.len();

    // For each GUID hash, we expect a BalanceAccount account and an associated
    // token account, so the max number of AccountInfos, less BASE_ACCOUNTS_LEN,
    // is 2 * account_guid_hashes.len().
    let expected_account_count = BASE_ACCOUNTS_LEN
        .checked_add(
            n_guid_hashes
                .checked_mul(2)
                .ok_or(WalletError::AmountOverflow)?,
        )
        .ok_or(WalletError::AmountOverflow)?;

    let accounts_vec = accounts.to_vec();

    if accounts_vec.len() != expected_account_count {
        msg!(
            "Expected {} accounts in instruction but got {}",
            expected_account_count,
            accounts_vec.len()
        );
        return Err(ProgramError::NotEnoughAccountKeys);
    }

    // get associated token accounts slice
    let mut index = BASE_ACCOUNTS_LEN;
    let mut offset = index
        .checked_add(n_guid_hashes)
        .ok_or(WalletError::AmountOverflow)?;

    let associated_token_account_infos = accounts_vec
        .get(index..offset)
        .ok_or(ProgramError::NotEnoughAccountKeys)?;

    // get BalanceAccount accounts slice
    index = offset;
    offset = index
        .checked_add(n_guid_hashes)
        .ok_or(WalletError::AmountOverflow)?;

    let balance_account_infos = accounts_vec
        .get(index..offset)
        .ok_or(ProgramError::NotEnoughAccountKeys)?;

    // get bump seed of payer BalanceAccount
    let payer_bump_seed = validate_balance_account_and_get_seed(
        payer_balance_account_info,
        payer_account_guid_hash,
        program_id,
    )?;

    // Between init and finalize, it is possible that all or some of the
    // associated token accounts were created by other concurrent multisig ops.
    // Therefore, we need to filter out any of these accounts, keeping track of
    // the array indices of "uncreated" or "new" associated token accounts only,
    // ignoring the others.
    let mut new_associated_token_account_indices = Vec::<usize>::with_capacity(n_guid_hashes);
    for (i, account_info) in associated_token_account_infos.iter().enumerate() {
        if *account_info.owner == Pubkey::default() {
            new_associated_token_account_indices.push(i);
        }
    }

    // ensure that the payer BalanceAccount has sufficient funds to cover the
    // creation of the new associated token accounts.
    let rent = Rent::get()?;
    let required_lamports = rent.minimum_balance(
        SPLAccount::LEN
            .checked_mul(new_associated_token_account_indices.len())
            .ok_or(WalletError::AmountOverflow)?,
    );
    if payer_balance_account_info.lamports() < required_lamports {
        msg!(
            "BalanceAccount {} has insufficient lamports to create associated token accounts",
            payer_balance_account_info.key
        );
        return Err(ProgramError::InsufficientFunds);
    }

    finalize_multisig_op(
        &multisig_op_account_info,
        &rent_collector_account_info,
        clock,
        MultisigOpParams::BalanceAccountEnableSplToken {
            wallet_address: *wallet_account_info.key,
            payer_account_guid_hash: *payer_account_guid_hash,
            account_guid_hashes: account_guid_hashes.clone(),
            token_mint: *token_mint_account_info.key,
        },
        || -> ProgramResult {
            // create associated token accounts for BalanceAccounts:
            for i in new_associated_token_account_indices.iter() {
                let balance_account_info = &balance_account_infos[*i];
                let associated_token_account_info = &associated_token_account_infos[*i];

                // creating account for account_guid_hashes[*i]
                invoke_signed(
                    &create_associated_token_account_instruction(
                        payer_balance_account_info,
                        associated_token_account_info,
                        balance_account_info,
                        token_mint_account_info,
                    ),
                    accounts,
                    &[&[&payer_account_guid_hash.to_bytes(), &[payer_bump_seed]]],
                )?;
            }
            Ok(())
        },
    )?;

    Ok(())
}
