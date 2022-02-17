use crate::handlers::{
    account_settings_update_handler, approval_disposition_handler,
    balance_account_creation_handler, balance_account_update_handler, dapp_book_update_handler,
    dapp_transaction_handler, init_wallet_handler, transfer_handler, update_signer_handler,
    wallet_config_policy_update_handler, wallet_update_handler, wrap_unwrap_handler,
};
use crate::instruction::ProgramInstruction;
use solana_program::{account_info::AccountInfo, entrypoint::ProgramResult, pubkey::Pubkey};

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
                init_wallet_handler::handle(program_id, accounts, &update)
            }

            ProgramInstruction::InitWalletUpdate { update } => {
                wallet_update_handler::init(program_id, accounts, &update)
            }

            ProgramInstruction::FinalizeWalletUpdate { update } => {
                wallet_update_handler::finalize(program_id, accounts, &update)
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
            } => balance_account_creation_handler::init(
                program_id,
                accounts,
                &account_guid_hash,
                &update,
            ),

            ProgramInstruction::FinalizeBalanceAccountCreation {
                account_guid_hash,
                update,
            } => balance_account_creation_handler::finalize(
                program_id,
                accounts,
                &account_guid_hash,
                &update,
            ),

            ProgramInstruction::InitBalanceAccountUpdate {
                account_guid_hash,
                update,
            } => balance_account_update_handler::init(
                program_id,
                accounts,
                &account_guid_hash,
                &update,
            ),

            ProgramInstruction::FinalizeBalanceAccountUpdate {
                account_guid_hash,
                update,
            } => balance_account_update_handler::finalize(
                program_id,
                accounts,
                &account_guid_hash,
                &update,
            ),

            ProgramInstruction::InitTransfer {
                account_guid_hash,
                amount,
                destination_name_hash,
            } => transfer_handler::init(
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
            } => transfer_handler::finalize(
                program_id,
                &accounts,
                &account_guid_hash,
                amount,
                token_mint,
            ),

            ProgramInstruction::SetApprovalDisposition {
                disposition,
                params_hash,
            } => approval_disposition_handler::handle(
                program_id,
                &accounts,
                disposition,
                params_hash,
            ),

            ProgramInstruction::InitWrapUnwrap {
                account_guid_hash,
                amount,
                direction,
            } => wrap_unwrap_handler::init(
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
            } => wrap_unwrap_handler::finalize(
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
            } => update_signer_handler::init(
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
            } => update_signer_handler::finalize(
                program_id,
                &accounts,
                slot_update_type,
                slot_id,
                signer,
            ),

            ProgramInstruction::InitDAppTransaction {
                ref account_guid_hash,
                dapp,
                instructions,
            } => dapp_transaction_handler::init(
                program_id,
                accounts,
                account_guid_hash,
                dapp,
                instructions,
            ),

            ProgramInstruction::FinalizeDAppTransaction {
                ref account_guid_hash,
                dapp,
                ref instructions,
            } => dapp_transaction_handler::finalize(
                program_id,
                accounts,
                account_guid_hash,
                dapp,
                instructions,
            ),

            ProgramInstruction::InitAccountSettingsUpdate {
                account_guid_hash,
                whitelist_enabled,
                dapps_enabled,
            } => account_settings_update_handler::init(
                program_id,
                &accounts,
                &account_guid_hash,
                whitelist_enabled,
                dapps_enabled,
            ),

            ProgramInstruction::FinalizeAccountSettingsUpdate {
                account_guid_hash,
                whitelist_enabled,
                dapps_enabled,
            } => account_settings_update_handler::finalize(
                program_id,
                &accounts,
                &account_guid_hash,
                whitelist_enabled,
                dapps_enabled,
            ),

            ProgramInstruction::InitDAppBookUpdate { update } => {
                dapp_book_update_handler::init(program_id, &accounts, &update)
            }

            ProgramInstruction::FinalizeDAppBookUpdate { update } => {
                dapp_book_update_handler::finalize(program_id, &accounts, &update)
            }
        }
    }
}
