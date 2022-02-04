use bitvec::macros::internal::funty::Fundamental;

use crate::error::WalletError;
use crate::handlers::utils::{
    calculate_expires, collect_remaining_balance, get_clock_from_next_account,
    next_program_account_info, validate_balance_account_and_get_seed,
};
use crate::model::balance_account::BalanceAccountGuidHash;
use crate::model::multisig_op::{MultisigOp, MultisigOpParams};
use crate::model::wallet::Wallet;
use solana_program::account_info::{next_account_info, AccountInfo};
use solana_program::entrypoint::ProgramResult;
use solana_program::instruction::Instruction;
use solana_program::msg;
use solana_program::program::invoke_signed;
use solana_program::program_error::ProgramError;
use solana_program::program_pack::Pack;
use solana_program::pubkey::Pubkey;
use spl_token::state::Account as SPLAccount;

pub fn init(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    account_guid_hash: &BalanceAccountGuidHash,
    instructions: Vec<Instruction>,
) -> ProgramResult {
    let accounts_iter = &mut accounts.iter();
    let multisig_op_account_info = next_program_account_info(accounts_iter, program_id)?;
    let wallet_account_info = next_program_account_info(accounts_iter, program_id)?;
    let initiator_account_info = next_account_info(accounts_iter)?;
    let clock = get_clock_from_next_account(accounts_iter)?;

    let wallet = Wallet::unpack(&wallet_account_info.data.borrow())?;
    let balance_account = wallet.get_balance_account(account_guid_hash)?;

    wallet.validate_transfer_initiator(balance_account, initiator_account_info)?;

    let mut multisig_op = MultisigOp::unpack_unchecked(&multisig_op_account_info.data.borrow())?;
    multisig_op.init(
        wallet.get_transfer_approvers_keys(balance_account),
        1,
        clock.unix_timestamp,
        calculate_expires(
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
                SPLAccount::unpack(&a.data.borrow())
                    .ok()
                    .map(|account_data| SplBalance {
                        account: *a.key,
                        token_mint: account_data.mint,
                        balance: account_data.amount,
                    })
            } else {
                None
            }
        })
        .collect()
}

fn balance_changes_from_simulation(
    starting_balances: Vec<u64>,
    starting_spl_balances: Vec<SplBalance>,
    ending_balances: Vec<u64>,
    ending_spl_balances: Vec<SplBalance>,
    accounts: &[AccountInfo],
) -> String {
    // compute just the changes to minimize compute budget spend
    let balance_changes: Vec<(u8, char, u64)> = starting_balances
        .into_iter()
        .enumerate()
        .filter_map(|(i, starting_balance)| {
            if ending_balances[i] > starting_balance {
                Some((i as u8, '+', ending_balances[i] - starting_balance))
            } else if ending_balances[i] < starting_balance {
                Some((i as u8, '-', starting_balance - ending_balances[i]))
            } else {
                None
            }
        })
        .collect();

    let spl_balance_changes: Vec<(u8, char, u64)> = ending_spl_balances
        .into_iter()
        .filter_map(|end| {
            let starting_balance = starting_spl_balances
                .iter()
                .find(|start| start.account == end.account && start.token_mint == end.token_mint)
                .map(|start| start.balance)
                .unwrap_or(0);
            if end.balance == starting_balance {
                None
            } else {
                let index = accounts
                    .iter()
                    .position(|a| *a.key == end.account)
                    .unwrap()
                    .as_u8();
                if end.balance > starting_balance {
                    Some((
                        index,
                        '+',
                        end.balance.checked_sub(starting_balance).unwrap(),
                    ))
                } else {
                    Some((
                        index,
                        '-',
                        starting_balance.checked_sub(end.balance).unwrap(),
                    ))
                }
            }
        })
        .collect();
    format!(
        "Simulation balance changes: {:?} {:?}",
        balance_changes, spl_balance_changes
    )
}

pub fn finalize(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    account_guid_hash: &BalanceAccountGuidHash,
    instructions: &Vec<Instruction>,
) -> ProgramResult {
    let accounts_iter = &mut accounts.iter();
    let multisig_op_account_info = next_program_account_info(accounts_iter, program_id)?;
    let wallet_account_info = next_program_account_info(accounts_iter, program_id)?;
    let balance_account = next_account_info(accounts_iter)?;
    let rent_collector_account_info = next_account_info(accounts_iter)?;
    let clock = get_clock_from_next_account(accounts_iter)?;

    if !rent_collector_account_info.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }

    let multisig_op = MultisigOp::unpack(&multisig_op_account_info.data.borrow())?;

    let expected_params = MultisigOpParams::DAppTransaction {
        wallet_address: *wallet_account_info.key,
        account_guid_hash: *account_guid_hash,
        instructions: instructions.clone(),
    };

    let is_approved = multisig_op
        .approved(&expected_params, &clock)
        .unwrap_or_else(|e| {
            msg!("Approval failed: {:?}", e);
            false
        });

    let bump_seed =
        validate_balance_account_and_get_seed(balance_account, account_guid_hash, program_id)?;

    let starting_balances: Vec<u64> = if is_approved {
        Vec::new()
    } else {
        account_balances(accounts)
    };

    let starting_spl_balances: Vec<SplBalance> = if is_approved {
        Vec::new()
    } else {
        spl_balances(accounts)
    };

    for instruction in instructions.iter() {
        invoke_signed(
            &instruction,
            &accounts,
            &[&[&account_guid_hash.to_bytes(), &[bump_seed]]],
        )?;
    }

    if is_approved {
        collect_remaining_balance(&multisig_op_account_info, &rent_collector_account_info)?;

        Ok(())
    } else {
        msg!(&balance_changes_from_simulation(
            starting_balances,
            starting_spl_balances,
            account_balances(accounts),
            spl_balances(accounts),
            accounts,
        ));
        Err(WalletError::SimulationFinished.into())
    }
}

struct SplBalance {
    account: Pubkey,
    token_mint: Pubkey,
    balance: u64,
}

#[test]
fn test_balance_changes() {
    assert_eq![
        "Simulation balance changes: [] []",
        balance_changes_from_simulation(vec![], vec![], vec![], vec![], &[])
    ];
    assert_eq![
        "Simulation balance changes: [(0, '+', 100)] []",
        balance_changes_from_simulation(vec![0], vec![], vec![100], vec![], &[])
    ];
    assert_eq![
        "Simulation balance changes: [(1, '-', 100)] []",
        balance_changes_from_simulation(vec![0, 100], vec![], vec![0, 0], vec![], &[])
    ];
    let account = Pubkey::new_unique();
    let owner = Pubkey::new_unique();
    let token_mint = Pubkey::new_unique();
    let mut account_lamports = 0;
    let mut account_data: [u8; 0] = [0; 0];
    let account_info = AccountInfo::new(
        &account,
        false,
        false,
        &mut account_lamports,
        &mut account_data,
        &owner,
        false,
        0,
    );

    assert_eq![
        "Simulation balance changes: [] [(0, '+', 100)]",
        balance_changes_from_simulation(
            vec![],
            vec![SplBalance {
                account,
                token_mint,
                balance: 0
            }],
            vec![],
            vec![SplBalance {
                account,
                token_mint,
                balance: 100
            }],
            &[account_info.clone()]
        )
    ];

    let other_account = Pubkey::new_unique();
    let mut other_account_info = account_info.clone();
    other_account_info.key = &other_account;

    assert_eq![
        "Simulation balance changes: [] [(1, '-', 100)]",
        balance_changes_from_simulation(
            vec![],
            vec![SplBalance {
                account,
                token_mint,
                balance: 200
            }],
            vec![],
            vec![SplBalance {
                account,
                token_mint,
                balance: 100
            }],
            &[other_account_info.clone(), account_info.clone()]
        )
    ];

    assert_eq![
        "Simulation balance changes: [] [(0, '+', 100)]",
        balance_changes_from_simulation(
            vec![],
            vec![SplBalance {
                account: other_account,
                token_mint,
                balance: 200
            }],
            vec![],
            vec![SplBalance {
                account,
                token_mint,
                balance: 100
            }],
            &[account_info.clone(), other_account_info.clone()]
        )
    ];
}
