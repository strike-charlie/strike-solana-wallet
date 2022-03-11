use std::convert::TryInto;
use std::mem::size_of;
use std::slice::Iter;
use std::time::Duration;

use bitvec::macros::internal::funty::Fundamental;
use bytes::BufMut;
use solana_program::hash::Hash;
use solana_program::program_error::ProgramError;
use solana_program::program_pack::Pack;
use solana_program::{instruction::AccountMeta, instruction::Instruction, pubkey::Pubkey};

use crate::constants::{GUID_HASH_BYTES, NAME_HASH_BYTES, PARAMS_HASH_BYTES, PUBKEY_BYTES};
use crate::model::address_book::{AddressBookEntry, AddressBookEntryNameHash, DAppBookEntry};
use crate::model::balance_account::{
    BalanceAccount, BalanceAccountGuidHash, BalanceAccountNameHash,
};
use crate::model::multisig_op::{
    ApprovalDisposition, BooleanSetting, SlotUpdateType, WrapDirection,
};
use crate::model::signer::Signer;
use crate::serialization_utils::{
    append_duration, append_optional_duration, append_optional_u8, pack_option, read_duration,
    read_fixed_size_array, read_optional_duration, read_optional_u8, read_slice, read_u16, read_u8,
    unpack_option,
};
use crate::utils::{unique_account_metas, SlotId};

// Instruction "tags" are sent as the first byte of each instruction
// and are mapped to corresponding ProgramInstructions to execute:
pub const TAG_INIT_WALLET: u8 = 0;
pub const TAG_INIT_BALANCE_ACCOUNT_CREATION: u8 = 3;
pub const TAG_FINALIZE_BALANCE_ACCOUNT_CREATION: u8 = 4;
pub const TAG_INIT_BALANCE_ACCOUNT_UPDATE: u8 = 5;
pub const TAG_FINALIZE_BALANCE_ACCOUNT_UPDATE: u8 = 6;
pub const TAG_INIT_TRANSFER: u8 = 7;
pub const TAG_FINALIZE_TRANSFER: u8 = 8;
pub const TAG_SET_APPROVAL_DISPOSITION: u8 = 9;
pub const TAG_INIT_WRAP_UNWRAP: u8 = 10;
pub const TAG_FINALIZE_WRAP_UNWRAP: u8 = 11;
pub const TAG_INIT_UPDATE_SIGNER: u8 = 12;
pub const TAG_FINALIZE_UPDATE_SIGNER: u8 = 13;
pub const TAG_INIT_WALLET_CONFIG_POLICY_UPDATE: u8 = 14;
pub const TAG_FINALIZE_WALLET_CONFIG_POLICY_UPDATE: u8 = 15;
pub const TAG_INIT_DAPP_TRANSACTION: u8 = 16;
pub const TAG_FINALIZE_DAPP_TRANSACTION: u8 = 17;
pub const TAG_INIT_ACCOUNT_SETTINGS_UPDATE: u8 = 18;
pub const TAG_FINALIZE_ACCOUNT_SETTINGS_UPDATE: u8 = 19;
pub const TAG_INIT_DAPP_BOOK_UPDATE: u8 = 20;
pub const TAG_FINALIZE_DAPP_BOOK_UPDATE: u8 = 21;
pub const TAG_INIT_ADDRESS_BOOK_UPDATE: u8 = 22;
pub const TAG_FINALIZE_ADDRESS_BOOK_UPDATE: u8 = 23;
pub const TAG_INIT_BALANCE_ACCOUNT_NAME_UPDATE: u8 = 24;
pub const TAG_FINALIZE_BALANCE_ACCOUNT_NAME_UPDATE: u8 = 25;
pub const TAG_INIT_BALANCE_ACCOUNT_POLICY_UPDATE: u8 = 26;
pub const TAG_FINALIZE_BALANCE_ACCOUNT_POLICY_UPDATE: u8 = 27;
pub const TAG_INIT_BALANCE_ACCOUNT_ENABLE_SPL_TOKEN: u8 = 28;
pub const TAG_FINALIZE_BALANCE_ACCOUNT_ENABLE_SPL_TOKEN: u8 = 29;

#[derive(Debug)]
pub enum ProgramInstruction {
    /// 0. `[writable]` The wallet account
    /// 1. `[signer]` The transaction assistant account
    InitWallet { initial_config: InitialWalletConfig },

    /// 0. `[writable]` The multisig operation account
    /// 1. `[]` The wallet account
    /// 2. `[signer]` The initiator account (either the transaction assistant or an approver)
    /// 3. `[]` The sysvar clock account
    InitBalanceAccountCreation {
        account_guid_hash: BalanceAccountGuidHash,
        creation_params: BalanceAccountCreation,
    },

    /// 0. `[writable]` The multisig operation account
    /// 1. `[writable]` The wallet account
    /// 2. `[signer]` The rent collector account
    FinalizeBalanceAccountCreation {
        account_guid_hash: BalanceAccountGuidHash,
        creation_params: BalanceAccountCreation,
    },

    /// 0. `[writable]` The multisig operation account
    /// 1. `[]` The wallet account
    /// 2. `[writable]` The source account
    /// 3. `[]` The destination account
    /// 4. `[signer]` The initiator account (either the transaction assistant or an approver)
    /// 5. `[]` The sysvar clock account
    /// 6. `[]` The token mint (for SPL transfers, use system account otherwise)
    /// 7. `[writable]` The destination token account (only used for SPL transfers)
    /// 8. `[signer, writable]` The fee payer, used if we need to create destination token account
    ///     for an SPL transfer and the source account does not have enough funds (only used for
    ///     SPL transfers)
    /// 9. `[]` The system program (only used for SPL transfers)
    /// 10. `[]` The SPL token program (only used for SPL transfers)
    /// 11. `[]` The Rent sysvar program (only used for SPL transfers)
    /// 12. `[]` The SPL associated token program (only used for SPL transfers)
    InitTransfer {
        account_guid_hash: BalanceAccountGuidHash,
        amount: u64,
        destination_name_hash: AddressBookEntryNameHash,
    },

    /// 0. `[writable]` The multisig operation account
    /// 1. `[signer]` The approver account
    /// 2. `[]` The sysvar clock account
    SetApprovalDisposition {
        disposition: ApprovalDisposition,
        params_hash: Hash,
    },

    /// 0. `[writable]` The multisig operation account
    /// 1. `[]` The wallet account
    /// 2. `[writable]` The source account
    /// 3. `[writable]` The destination account
    /// 4. `[]` The system program
    /// 5. `[signer]` The rent collector account
    /// 6. `[]` The sysvar clock account
    /// 7. `[writable]` The source token account, if this is an SPL transfer
    /// 8. `[writable]` The destination token account, if this is an SPL transfer
    /// 9. `[]` The SPL token program account, if this is an SPL transfer
    /// 10. `[]` The token mint authority, if this is an SPL transfer
    FinalizeTransfer {
        account_guid_hash: BalanceAccountGuidHash,
        amount: u64,
        token_mint: Pubkey,
    },

    /// 0. `[writable]` The multisig operation account
    /// 1. `[]` The wallet account
    /// 2. `[writable]` The balance account
    /// 3. `[writable]` The associated wrapped SOL account
    /// 4. `[]` The native mint account
    /// 5. `[signer]` The initiator account (either the transaction assistant or an approver)
    /// 6. `[]` The sysvar clock account
    /// 7. `[]` The system program
    /// 8. `[]` The SPL token program
    /// 9. `[]` The Rent sysvar program
    /// 10. `[]` The SPL associated token program
    InitWrapUnwrap {
        account_guid_hash: BalanceAccountGuidHash,
        amount: u64,
        direction: WrapDirection,
    },

    /// 0. `[writable]` The multisig operation account
    /// 1. `[]` The wallet account
    /// 2. `[writable]` The balance account
    /// 3. `[]` The system program
    /// 4. `[signer]` The rent collector account
    /// 5. `[]` The sysvar clock account
    /// 6. `[writable]` The wrapped SOL token account
    /// 7. `[]` The SPL token account
    FinalizeWrapUnwrap {
        account_guid_hash: BalanceAccountGuidHash,
        amount: u64,
        direction: WrapDirection,
    },
    /// 0. `[writable]` The multisig operation account
    /// 1. `[]` The wallet account
    /// 2. `[signer]` The initiator account (either the transaction assistant or an approver)
    /// 3. `[]` The sysvar clock account
    InitUpdateSigner {
        slot_update_type: SlotUpdateType,
        slot_id: SlotId<Signer>,
        signer: Signer,
    },

    /// 0. `[writable]` The multisig operation account
    /// 1. `[writable]` The wallet account
    /// 2. `[signer]` The rent collector account
    FinalizeUpdateSigner {
        slot_update_type: SlotUpdateType,
        slot_id: SlotId<Signer>,
        signer: Signer,
    },

    /// 0  `[writable]` The multisig operation account
    /// 1. `[writable]` The wallet account
    /// 2. `[signer]` The initiator account (either the transaction assistant or an approver)
    /// 3. `[]` The sysvar clock account
    InitWalletConfigPolicyUpdate { update: WalletConfigPolicyUpdate },

    /// 0  `[writable]` The multisig operation account
    /// 1. `[writable]` The wallet account
    /// 2. `[signer]` The rent collector account
    FinalizeWalletConfigPolicyUpdate { update: WalletConfigPolicyUpdate },

    /// 0. `[writable]` The multisig operation account
    /// 1. `[]` The wallet account
    /// 2. `[signer]` The initiator account (either the transaction assistant or an approver)
    /// 3. `[]` The sysvar clock account
    InitDAppTransaction {
        account_guid_hash: BalanceAccountGuidHash,
        instructions: Vec<Instruction>,
        dapp: DAppBookEntry,
    },

    /// 0. `[writable]` The multisig operation account
    /// 1. `[]` The wallet account
    /// 2. `[writable]` The balance account
    /// 3. `[signer]` The rent collector account
    /// 4. `[]` The sysvar clock account
    FinalizeDAppTransaction {
        account_guid_hash: BalanceAccountGuidHash,
        instructions: Vec<Instruction>,
        dapp: DAppBookEntry,
    },

    /// 0  `[writable]` The multisig operation account
    /// 1. `[]` The wallet account
    /// 2. `[signer]` The initiator account (either the transaction assistant or an approver)
    /// 3. `[]` The sysvar clock account
    InitAccountSettingsUpdate {
        account_guid_hash: BalanceAccountGuidHash,
        whitelist_enabled: Option<BooleanSetting>,
        dapps_enabled: Option<BooleanSetting>,
    },

    /// 0  `[writable]` The multisig operation account
    /// 1. `[writable]` The wallet account
    /// 2. `[signer]` The rent collector account
    FinalizeAccountSettingsUpdate {
        account_guid_hash: BalanceAccountGuidHash,
        whitelist_enabled: Option<BooleanSetting>,
        dapps_enabled: Option<BooleanSetting>,
    },

    /// 0. `[writable]` The multisig operation account
    /// 1. `[]` The wallet account
    /// 2. `[signer]` The initiator account (either the transaction assistant or an approver)
    /// 3. `[]` The sysvar clock account
    InitDAppBookUpdate { update: DAppBookUpdate },

    /// 0. `[writable]` The multisig operation account
    /// 1. `[writable]` The wallet account
    /// 2. `[signer]` The rent collector account
    /// 3. `[]` The sysvar clock account
    FinalizeDAppBookUpdate { update: DAppBookUpdate },

    /// 0. `[writable]` The multisig operation account
    /// 1. `[]` The wallet account
    /// 2. `[signer]` The initiator account (either the transaction assistant or an approver)
    /// 3. `[]` The sysvar clock account
    InitAddressBookUpdate { update: AddressBookUpdate },

    /// 0. `[writable]` The multisig operation account
    /// 1. `[writable]` The wallet account
    /// 2. `[signer]` The rent collector account
    /// 3. `[]` The sysvar clock account
    FinalizeAddressBookUpdate { update: AddressBookUpdate },

    /// 0. `[writable]` The multisig operation account
    /// 1. `[]` The wallet account
    /// 2. `[signer]` The initiator account (either the transaction assistant or an approver)
    /// 3. `[]` The sysvar clock account
    InitBalanceAccountNameUpdate {
        account_guid_hash: BalanceAccountGuidHash,
        account_name_hash: BalanceAccountNameHash,
    },

    /// 0. `[writable]` The multisig operation account
    /// 1. `[writable]` The wallet account
    /// 2. `[signer]` The rent collector account
    /// 3. `[]` The sysvar clock account
    FinalizeBalanceAccountNameUpdate {
        account_guid_hash: BalanceAccountGuidHash,
        account_name_hash: BalanceAccountNameHash,
    },

    /// 0. `[writable]` The multisig operation account
    /// 1. `[writable]` The wallet account
    /// 2. `[signer]` The initiator account (either the transaction assistant or an approver)
    /// 3. `[]` The sysvar clock account
    InitBalanceAccountPolicyUpdate {
        account_guid_hash: BalanceAccountGuidHash,
        update: BalanceAccountPolicyUpdate,
    },

    /// 0. `[writable]` The multisig operation account
    /// 1. `[writable]` The wallet account
    /// 2. `[signer]` The rent collector account
    /// 3. `[]` The sysvar clock account
    FinalizeBalanceAccountPolicyUpdate {
        account_guid_hash: BalanceAccountGuidHash,
        update: BalanceAccountPolicyUpdate,
    },

    /// 0. `[writable]` The multisig operation account
    /// 1. `[writable]` The wallet account
    /// 2. `[signer]` The rent collector account
    /// 3. `[]` The token mint account enabled for the given BalanceAccounts.
    /// 4. `[]` The sysvar clock account
    /// 5..N `[]` One or more associated token accounts, corresponding in number
    ///           and order to the account GUID hashes.
    InitBalanceAccountEnableSplToken {
        payer_account_guid_hash: BalanceAccountGuidHash,
        account_guid_hashes: Vec<BalanceAccountGuidHash>,
    },

    /// 0. `[writable]` The multisig operation account
    /// 1. `[writable]` The wallet account
    /// 2. `[signer]` The rent collector account
    /// 3. `[]` The token mint account enabled for the given BalanceAccounts.
    /// 4. `[]` The rent-collector balance account.
    /// 5. `[]` The sysvar clock account
    /// 6. `[]` The system program (only by SPL associated token program)
    /// 7. `[]` The SPL associated token program
    /// 8. `[]` The SPL token program (only used by SPL associated token program)
    /// 9. `[]` The Rent sysvar program
    /// 10..N `[]` One or more associated token accounts.
    /// N..M  `[]` One or more BalanceAccount accounts, corresponding in number
    ///            and order to the associated token accounts.
    FinalizeBalanceAccountEnableSplToken {
        payer_account_guid_hash: BalanceAccountGuidHash,
        account_guid_hashes: Vec<BalanceAccountGuidHash>,
    },
}

impl ProgramInstruction {
    /// Serialize a ProgramInstruction to a byte vector.
    pub fn pack(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(size_of::<Self>());
        match self {
            &ProgramInstruction::InitWallet {
                initial_config: ref update,
            } => {
                let mut update_bytes: Vec<u8> = Vec::new();
                update.pack(&mut update_bytes);
                buf.push(TAG_INIT_WALLET);
                buf.extend_from_slice(&update_bytes);
            }
            &ProgramInstruction::InitBalanceAccountCreation {
                ref account_guid_hash,
                ref creation_params,
            } => {
                let mut update_bytes: Vec<u8> = Vec::new();
                creation_params.pack(&mut update_bytes);
                buf.push(TAG_INIT_BALANCE_ACCOUNT_CREATION);
                buf.extend_from_slice(account_guid_hash.to_bytes());
                buf.extend_from_slice(&update_bytes);
            }
            &ProgramInstruction::FinalizeBalanceAccountCreation {
                ref account_guid_hash,
                ref creation_params,
            } => {
                let mut update_bytes: Vec<u8> = Vec::new();
                creation_params.pack(&mut update_bytes);
                buf.push(TAG_FINALIZE_BALANCE_ACCOUNT_CREATION);
                buf.extend_from_slice(account_guid_hash.to_bytes());
                buf.extend_from_slice(&update_bytes);
            }
            &ProgramInstruction::InitTransfer {
                ref account_guid_hash,
                ref amount,
                ref destination_name_hash,
            } => {
                buf.push(TAG_INIT_TRANSFER);
                buf.extend_from_slice(account_guid_hash.to_bytes());
                buf.extend_from_slice(&amount.to_le_bytes());
                buf.extend_from_slice(destination_name_hash.to_bytes());
            }
            &ProgramInstruction::FinalizeTransfer {
                ref account_guid_hash,
                ref amount,
                ref token_mint,
            } => {
                buf.push(TAG_FINALIZE_TRANSFER);
                buf.extend_from_slice(account_guid_hash.to_bytes());
                buf.extend_from_slice(&amount.to_le_bytes());
                buf.extend_from_slice(&token_mint.to_bytes());
                buf.push(0);
            }
            &ProgramInstruction::SetApprovalDisposition {
                ref disposition,
                ref params_hash,
            } => {
                buf.push(TAG_SET_APPROVAL_DISPOSITION);
                buf.push(disposition.to_u8());
                buf.extend_from_slice(params_hash.as_ref());
            }
            &ProgramInstruction::InitWrapUnwrap {
                ref account_guid_hash,
                ref amount,
                ref direction,
            } => {
                buf.push(TAG_INIT_WRAP_UNWRAP);
                buf.extend_from_slice(&account_guid_hash.to_bytes());
                buf.extend_from_slice(&amount.to_le_bytes());
                buf.push(direction.to_u8());
            }
            &ProgramInstruction::FinalizeWrapUnwrap {
                ref account_guid_hash,
                ref amount,
                ref direction,
            } => {
                buf.push(TAG_FINALIZE_WRAP_UNWRAP);
                buf.extend_from_slice(&account_guid_hash.to_bytes());
                buf.extend_from_slice(&amount.to_le_bytes());
                buf.push(direction.to_u8());
            }
            &ProgramInstruction::InitUpdateSigner {
                ref slot_update_type,
                ref slot_id,
                ref signer,
            } => {
                buf.push(TAG_INIT_UPDATE_SIGNER);
                buf.push(slot_update_type.to_u8());
                buf.push(slot_id.value as u8);
                buf.extend_from_slice(signer.key.as_ref());
            }
            &ProgramInstruction::FinalizeUpdateSigner {
                ref slot_update_type,
                ref slot_id,
                ref signer,
            } => {
                buf.push(TAG_FINALIZE_UPDATE_SIGNER);
                buf.push(slot_update_type.to_u8());
                buf.push(slot_id.value as u8);
                buf.extend_from_slice(signer.key.as_ref());
            }
            &ProgramInstruction::InitWalletConfigPolicyUpdate { ref update } => {
                let mut update_bytes: Vec<u8> = Vec::new();
                update.pack(&mut update_bytes);
                buf.push(TAG_INIT_WALLET_CONFIG_POLICY_UPDATE);
                buf.extend_from_slice(&update_bytes);
            }
            &ProgramInstruction::FinalizeWalletConfigPolicyUpdate { ref update } => {
                let mut update_bytes: Vec<u8> = Vec::new();
                update.pack(&mut update_bytes);
                buf.push(TAG_FINALIZE_WALLET_CONFIG_POLICY_UPDATE);
                buf.extend_from_slice(&update_bytes);
            }
            &ProgramInstruction::InitDAppTransaction {
                ref account_guid_hash,
                ref dapp,
                ref instructions,
            } => {
                buf.push(TAG_INIT_DAPP_TRANSACTION);
                buf.extend_from_slice(&account_guid_hash.to_bytes());
                let mut buf2 = vec![0; DAppBookEntry::LEN];
                dapp.pack_into_slice(buf2.as_mut_slice());
                buf.extend_from_slice(&buf2[..]);
                let account_metas = unique_account_metas(instructions, &Vec::new());
                buf.put_u8(account_metas.len().as_u8());
                for account_meta in account_metas.iter() {
                    let mut account_meta_buf = vec![0; 1 + Signer::LEN];
                    account_meta_buf[0] = 0;
                    if account_meta.is_signer {
                        account_meta_buf[0] |= 2;
                    }
                    if account_meta.is_writable {
                        account_meta_buf[0] |= 1;
                    }
                    Signer::new(account_meta.pubkey)
                        .pack_into_slice(&mut account_meta_buf[1..1 + Signer::LEN]);
                    buf.extend_from_slice(account_meta_buf.as_slice());
                }
                buf.put_u16_le(instructions.len() as u16);
                for instruction in instructions.iter() {
                    append_instruction(instruction, &account_metas, &mut buf);
                }
            }
            &ProgramInstruction::FinalizeDAppTransaction {
                ref account_guid_hash,
                ref dapp,
                ref instructions,
            } => {
                buf.push(TAG_FINALIZE_DAPP_TRANSACTION);
                buf.extend_from_slice(&account_guid_hash.to_bytes());
                let mut buf2 = vec![0; DAppBookEntry::LEN];
                dapp.pack_into_slice(buf2.as_mut_slice());
                buf.extend_from_slice(&buf2[..]);
                let account_metas = unique_account_metas(instructions, &Vec::new());
                buf.put_u8(account_metas.len().as_u8());
                for account_meta in account_metas.iter() {
                    let mut account_meta_buf = vec![0; 1 + Signer::LEN];
                    account_meta_buf[0] = 0;
                    if account_meta.is_signer {
                        account_meta_buf[0] |= 2;
                    }
                    if account_meta.is_writable {
                        account_meta_buf[0] |= 1;
                    }
                    Signer::new(account_meta.pubkey)
                        .pack_into_slice(&mut account_meta_buf[1..1 + Signer::LEN]);
                    buf.extend_from_slice(account_meta_buf.as_slice());
                }
                buf.put_u16_le(instructions.len() as u16);
                for instruction in instructions.iter() {
                    append_instruction(instruction, &account_metas, &mut buf);
                }
            }
            &ProgramInstruction::InitAccountSettingsUpdate {
                ref account_guid_hash,
                ref whitelist_enabled,
                ref dapps_enabled,
            } => {
                buf.push(TAG_INIT_ACCOUNT_SETTINGS_UPDATE);
                buf.extend_from_slice(&account_guid_hash.to_bytes());
                pack_option(whitelist_enabled.as_ref(), &mut buf);
                pack_option(dapps_enabled.as_ref(), &mut buf);
            }
            &ProgramInstruction::FinalizeAccountSettingsUpdate {
                ref account_guid_hash,
                ref whitelist_enabled,
                ref dapps_enabled,
            } => {
                buf.push(TAG_FINALIZE_ACCOUNT_SETTINGS_UPDATE);
                buf.extend_from_slice(&account_guid_hash.to_bytes());
                pack_option(whitelist_enabled.as_ref(), &mut buf);
                pack_option(dapps_enabled.as_ref(), &mut buf);
            }
            &ProgramInstruction::InitDAppBookUpdate { ref update } => {
                buf.push(TAG_INIT_DAPP_BOOK_UPDATE);
                let mut update_bytes: Vec<u8> = Vec::new();
                update.pack(&mut update_bytes);
                buf.extend_from_slice(&update_bytes);
            }
            &ProgramInstruction::FinalizeDAppBookUpdate { ref update } => {
                buf.push(TAG_FINALIZE_DAPP_BOOK_UPDATE);
                let mut update_bytes: Vec<u8> = Vec::new();
                update.pack(&mut update_bytes);
                buf.extend_from_slice(&update_bytes);
            }
            &ProgramInstruction::InitAddressBookUpdate { ref update } => {
                let mut update_bytes: Vec<u8> = Vec::new();
                update.pack(&mut update_bytes);
                buf.push(TAG_INIT_ADDRESS_BOOK_UPDATE);
                buf.extend_from_slice(&update_bytes);
            }
            &ProgramInstruction::FinalizeAddressBookUpdate { ref update } => {
                let mut update_bytes: Vec<u8> = Vec::new();
                update.pack(&mut update_bytes);
                buf.push(TAG_FINALIZE_ADDRESS_BOOK_UPDATE);
                buf.extend_from_slice(&update_bytes);
            }
            &ProgramInstruction::InitBalanceAccountNameUpdate {
                ref account_guid_hash,
                ref account_name_hash,
            } => {
                buf.push(TAG_INIT_BALANCE_ACCOUNT_NAME_UPDATE);
                buf.extend_from_slice(account_guid_hash.to_bytes());
                buf.extend_from_slice(account_name_hash.to_bytes());
            }
            &ProgramInstruction::FinalizeBalanceAccountNameUpdate {
                ref account_guid_hash,
                ref account_name_hash,
            } => {
                buf.push(TAG_FINALIZE_BALANCE_ACCOUNT_NAME_UPDATE);
                buf.extend_from_slice(account_guid_hash.to_bytes());
                buf.extend_from_slice(account_name_hash.to_bytes());
            }
            &ProgramInstruction::InitBalanceAccountPolicyUpdate {
                ref account_guid_hash,
                ref update,
            } => {
                let mut update_bytes: Vec<u8> = Vec::new();
                update.pack(&mut update_bytes);
                buf.push(TAG_INIT_BALANCE_ACCOUNT_POLICY_UPDATE);
                buf.extend_from_slice(account_guid_hash.to_bytes());
                buf.extend_from_slice(&update_bytes);
            }
            &ProgramInstruction::FinalizeBalanceAccountPolicyUpdate {
                ref account_guid_hash,
                ref update,
            } => {
                let mut update_bytes: Vec<u8> = Vec::new();
                update.pack(&mut update_bytes);
                buf.push(TAG_FINALIZE_BALANCE_ACCOUNT_POLICY_UPDATE);
                buf.extend_from_slice(account_guid_hash.to_bytes());
                buf.extend_from_slice(&update_bytes);
            }
            &ProgramInstruction::InitBalanceAccountEnableSplToken {
                ref payer_account_guid_hash,
                ref account_guid_hashes,
            } => {
                buf.push(TAG_INIT_BALANCE_ACCOUNT_ENABLE_SPL_TOKEN);
                buf.extend_from_slice(payer_account_guid_hash.to_bytes());
                pack_balance_account_guid_hash_vec(account_guid_hashes, &mut buf);
            }
            &ProgramInstruction::FinalizeBalanceAccountEnableSplToken {
                ref payer_account_guid_hash,
                ref account_guid_hashes,
            } => {
                buf.push(TAG_FINALIZE_BALANCE_ACCOUNT_ENABLE_SPL_TOKEN);
                buf.extend_from_slice(payer_account_guid_hash.to_bytes());
                pack_balance_account_guid_hash_vec(account_guid_hashes, &mut buf);
            }
        }
        buf
    }

    /// Deserialize a byte buffer to ProgramInstruction.
    pub fn unpack(input: &[u8]) -> Result<Self, ProgramError> {
        let (tag, rest) = input
            .split_first()
            .ok_or(ProgramError::InvalidInstructionData)?;

        Ok(match *tag {
            TAG_INIT_WALLET => Self::unpack_init_wallet_instruction(rest)?,

            TAG_INIT_BALANCE_ACCOUNT_CREATION => {
                Self::unpack_init_balance_account_creation_instruction(rest)?
            }
            TAG_FINALIZE_BALANCE_ACCOUNT_CREATION => {
                Self::unpack_finalize_balance_account_creation_instruction(rest)?
            }
            TAG_INIT_TRANSFER => Self::unpack_init_transfer_for_approval_instruction(rest)?,

            TAG_FINALIZE_TRANSFER => Self::unpack_finalize_transfer_instruction(rest)?,

            TAG_SET_APPROVAL_DISPOSITION => {
                Self::unpack_set_approval_disposition_instruction(rest)?
            }
            TAG_INIT_WRAP_UNWRAP => Self::unpack_init_wrap_unwrap_instruction(rest)?,

            TAG_FINALIZE_WRAP_UNWRAP => Self::unpack_finalize_wrap_unwrap_instruction(rest)?,

            TAG_INIT_UPDATE_SIGNER => Self::unpack_init_update_signer_instruction(rest)?,

            TAG_FINALIZE_UPDATE_SIGNER => Self::unpack_finalize_update_signer_instruction(rest)?,

            TAG_INIT_WALLET_CONFIG_POLICY_UPDATE => {
                Self::unpack_init_wallet_config_policy_update_instruction(rest)?
            }
            TAG_FINALIZE_WALLET_CONFIG_POLICY_UPDATE => {
                Self::unpack_finalize_wallet_config_policy_update_instruction(rest)?
            }
            TAG_INIT_DAPP_TRANSACTION => Self::unpack_init_dapp_transaction_instruction(rest)?,

            TAG_FINALIZE_DAPP_TRANSACTION => {
                Self::unpack_finalize_dapp_transaction_instruction(rest)?
            }
            TAG_INIT_ACCOUNT_SETTINGS_UPDATE => {
                Self::unpack_init_account_settings_update_instruction(rest)?
            }
            TAG_FINALIZE_ACCOUNT_SETTINGS_UPDATE => {
                Self::unpack_finalize_account_settings_update_instruction(rest)?
            }
            TAG_INIT_DAPP_BOOK_UPDATE => Self::unpack_init_dapp_book_update_instruction(rest)?,

            TAG_FINALIZE_DAPP_BOOK_UPDATE => {
                Self::unpack_finalize_dapp_book_update_instruction(rest)?
            }
            TAG_INIT_ADDRESS_BOOK_UPDATE => {
                Self::unpack_init_address_book_update_instruction(rest)?
            }
            TAG_FINALIZE_ADDRESS_BOOK_UPDATE => {
                Self::unpack_finalize_address_book_update_instruction(rest)?
            }
            TAG_INIT_BALANCE_ACCOUNT_NAME_UPDATE => {
                Self::unpack_init_balance_account_name_update_instruction(rest)?
            }
            TAG_FINALIZE_BALANCE_ACCOUNT_NAME_UPDATE => {
                Self::unpack_finalize_balance_account_name_update_instruction(rest)?
            }
            TAG_INIT_BALANCE_ACCOUNT_POLICY_UPDATE => {
                Self::unpack_init_balance_account_policy_update_instruction(rest)?
            }
            TAG_FINALIZE_BALANCE_ACCOUNT_POLICY_UPDATE => {
                Self::unpack_finalize_balance_account_policy_update_instruction(rest)?
            }
            TAG_INIT_BALANCE_ACCOUNT_ENABLE_SPL_TOKEN => {
                Self::unpack_init_balance_account_enable_spl_token(rest)?
            }
            TAG_FINALIZE_BALANCE_ACCOUNT_ENABLE_SPL_TOKEN => {
                Self::unpack_finalize_balance_account_enable_spl_token(rest)?
            }
            _ => return Err(ProgramError::InvalidInstructionData),
        })
    }

    fn unpack_init_wallet_instruction(bytes: &[u8]) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::InitWallet {
            initial_config: InitialWalletConfig::unpack(bytes)?,
        })
    }

    fn unpack_init_balance_account_creation_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::InitBalanceAccountCreation {
            account_guid_hash: unpack_account_guid_hash(bytes)?,
            creation_params: BalanceAccountCreation::unpack(
                bytes
                    .get(GUID_HASH_BYTES..)
                    .ok_or(ProgramError::InvalidInstructionData)?,
            )?,
        })
    }

    fn unpack_finalize_balance_account_creation_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::FinalizeBalanceAccountCreation {
            account_guid_hash: unpack_account_guid_hash(bytes)?,
            creation_params: BalanceAccountCreation::unpack(
                bytes
                    .get(GUID_HASH_BYTES..)
                    .ok_or(ProgramError::InvalidInstructionData)?,
            )?,
        })
    }

    fn unpack_init_balance_account_policy_update_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::InitBalanceAccountPolicyUpdate {
            account_guid_hash: unpack_account_guid_hash(bytes)?,
            update: BalanceAccountPolicyUpdate::unpack(
                bytes
                    .get(GUID_HASH_BYTES..)
                    .ok_or(ProgramError::InvalidInstructionData)?,
            )?,
        })
    }

    fn unpack_finalize_balance_account_policy_update_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::FinalizeBalanceAccountPolicyUpdate {
            account_guid_hash: unpack_account_guid_hash(bytes)?,
            update: BalanceAccountPolicyUpdate::unpack(
                bytes
                    .get(GUID_HASH_BYTES..)
                    .ok_or(ProgramError::InvalidInstructionData)?,
            )?,
        })
    }

    fn unpack_init_transfer_for_approval_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        let account_guid_hash = unpack_account_guid_hash(bytes)?;

        let amount = bytes
            .get(GUID_HASH_BYTES..GUID_HASH_BYTES + 8)
            .and_then(|slice| slice.try_into().ok())
            .map(u64::from_le_bytes)
            .ok_or(ProgramError::InvalidInstructionData)?;

        let byte_offset = GUID_HASH_BYTES + 8;
        let destination_name_hash = bytes
            .get(byte_offset..byte_offset + NAME_HASH_BYTES)
            .and_then(|slice| {
                slice
                    .try_into()
                    .ok()
                    .map(|bytes| AddressBookEntryNameHash::new(bytes))
            })
            .ok_or(ProgramError::InvalidInstructionData)?;

        Ok(Self::InitTransfer {
            account_guid_hash,
            amount,
            destination_name_hash,
        })
    }

    fn unpack_set_approval_disposition_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        let (disposition, rest) = bytes
            .split_first()
            .ok_or(ProgramError::InvalidInstructionData)?;
        Ok(Self::SetApprovalDisposition {
            disposition: ApprovalDisposition::from_u8(*disposition),
            params_hash: Hash::new_from_array(
                rest.get(0..PARAMS_HASH_BYTES)
                    .and_then(|slice| slice.try_into().ok())
                    .ok_or(ProgramError::InvalidInstructionData)?,
            ),
        })
    }

    fn unpack_finalize_transfer_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::FinalizeTransfer {
            account_guid_hash: unpack_account_guid_hash(bytes)?,
            amount: bytes
                .get(GUID_HASH_BYTES..GUID_HASH_BYTES + 8)
                .and_then(|slice| slice.try_into().ok())
                .map(u64::from_le_bytes)
                .ok_or(ProgramError::InvalidInstructionData)?,
            token_mint: unpack_public_key(bytes, GUID_HASH_BYTES + 8)?,
        })
    }

    fn unpack_init_wrap_unwrap_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        if let Some(direction) = bytes.get(40) {
            Ok(Self::InitWrapUnwrap {
                account_guid_hash: unpack_account_guid_hash(bytes)?,
                amount: bytes
                    .get(GUID_HASH_BYTES..GUID_HASH_BYTES + 8)
                    .and_then(|slice| slice.try_into().ok())
                    .map(u64::from_le_bytes)
                    .ok_or(ProgramError::InvalidInstructionData)?,
                direction: WrapDirection::from_u8(*direction),
            })
        } else {
            Err(ProgramError::InvalidInstructionData)
        }
    }

    fn unpack_finalize_wrap_unwrap_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        if let Some(direction) = bytes.get(40) {
            Ok(Self::FinalizeWrapUnwrap {
                account_guid_hash: unpack_account_guid_hash(bytes)?,
                amount: bytes
                    .get(GUID_HASH_BYTES..GUID_HASH_BYTES + 8)
                    .and_then(|slice| slice.try_into().ok())
                    .map(u64::from_le_bytes)
                    .ok_or(ProgramError::InvalidInstructionData)?,
                direction: WrapDirection::from_u8(*direction),
            })
        } else {
            Err(ProgramError::InvalidInstructionData)
        }
    }

    fn unpack_init_update_signer_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        let (slot_update_type, rest) = bytes
            .split_first()
            .ok_or(ProgramError::InvalidInstructionData)?;
        let (slot_id, rest) = rest
            .split_first()
            .ok_or(ProgramError::InvalidInstructionData)?;
        Ok(Self::InitUpdateSigner {
            slot_update_type: SlotUpdateType::from_u8(*slot_update_type),
            slot_id: SlotId::new(*slot_id as usize),
            signer: Signer::unpack_from_slice(rest)?,
        })
    }

    fn unpack_finalize_update_signer_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        let (slot_update_type, rest) = bytes
            .split_first()
            .ok_or(ProgramError::InvalidInstructionData)?;
        let (slot_id, rest) = rest
            .split_first()
            .ok_or(ProgramError::InvalidInstructionData)?;
        Ok(Self::FinalizeUpdateSigner {
            slot_update_type: SlotUpdateType::from_u8(*slot_update_type),
            slot_id: SlotId::new(*slot_id as usize),
            signer: Signer::unpack_from_slice(rest)?,
        })
    }

    fn unpack_init_wallet_config_policy_update_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::InitWalletConfigPolicyUpdate {
            update: WalletConfigPolicyUpdate::unpack(bytes)?,
        })
    }

    fn unpack_finalize_wallet_config_policy_update_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::FinalizeWalletConfigPolicyUpdate {
            update: WalletConfigPolicyUpdate::unpack(bytes)?,
        })
    }

    fn unpack_init_dapp_transaction_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        let iter = &mut bytes.into_iter();
        let account_guid_hash = unpack_account_guid_hash(
            read_slice(iter, GUID_HASH_BYTES).ok_or(ProgramError::InvalidInstructionData)?,
        )?;
        let dapp = DAppBookEntry::unpack_from_slice(
            read_slice(iter, DAppBookEntry::LEN).ok_or(ProgramError::InvalidInstructionData)?,
        )?;
        Ok(Self::InitDAppTransaction {
            account_guid_hash,
            dapp,
            instructions: read_instructions(iter)?,
        })
    }

    fn unpack_finalize_dapp_transaction_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        let iter = &mut bytes.into_iter();
        let account_guid_hash = unpack_account_guid_hash(
            read_slice(iter, GUID_HASH_BYTES).ok_or(ProgramError::InvalidInstructionData)?,
        )?;
        let dapp = DAppBookEntry::unpack_from_slice(
            read_slice(iter, DAppBookEntry::LEN).ok_or(ProgramError::InvalidInstructionData)?,
        )?;
        Ok(Self::FinalizeDAppTransaction {
            account_guid_hash,
            dapp,
            instructions: read_instructions(iter)?,
        })
    }

    fn unpack_init_account_settings_update_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        let iter = &mut bytes.into_iter();
        Ok(Self::InitAccountSettingsUpdate {
            account_guid_hash: unpack_account_guid_hash(
                read_slice(iter, GUID_HASH_BYTES).ok_or(ProgramError::InvalidInstructionData)?,
            )?,
            whitelist_enabled: unpack_option::<BooleanSetting>(iter)?,
            dapps_enabled: unpack_option::<BooleanSetting>(iter)?,
        })
    }

    fn unpack_finalize_account_settings_update_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        let iter = &mut bytes.into_iter();
        Ok(Self::FinalizeAccountSettingsUpdate {
            account_guid_hash: unpack_account_guid_hash(
                read_slice(iter, GUID_HASH_BYTES).ok_or(ProgramError::InvalidInstructionData)?,
            )?,
            whitelist_enabled: unpack_option::<BooleanSetting>(iter)?,
            dapps_enabled: unpack_option::<BooleanSetting>(iter)?,
        })
    }

    fn unpack_init_dapp_book_update_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::InitDAppBookUpdate {
            update: DAppBookUpdate::unpack(bytes)?,
        })
    }

    fn unpack_finalize_dapp_book_update_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::FinalizeDAppBookUpdate {
            update: DAppBookUpdate::unpack(bytes)?,
        })
    }

    fn unpack_init_address_book_update_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::InitAddressBookUpdate {
            update: AddressBookUpdate::unpack(bytes)?,
        })
    }

    fn unpack_finalize_address_book_update_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::FinalizeAddressBookUpdate {
            update: AddressBookUpdate::unpack(bytes)?,
        })
    }

    fn unpack_init_balance_account_name_update_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::InitBalanceAccountNameUpdate {
            account_guid_hash: unpack_account_guid_hash(bytes)?,
            account_name_hash: unpack_account_name_hash(
                bytes
                    .get(GUID_HASH_BYTES..)
                    .ok_or(ProgramError::InvalidInstructionData)?,
            )?,
        })
    }

    fn unpack_finalize_balance_account_name_update_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::FinalizeBalanceAccountNameUpdate {
            account_guid_hash: unpack_account_guid_hash(bytes)?,
            account_name_hash: unpack_account_name_hash(
                bytes
                    .get(GUID_HASH_BYTES..)
                    .ok_or(ProgramError::InvalidInstructionData)?,
            )?,
        })
    }

    fn unpack_init_balance_account_enable_spl_token(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::InitBalanceAccountEnableSplToken {
            payer_account_guid_hash: unpack_account_guid_hash(
                bytes
                    .get(0..GUID_HASH_BYTES)
                    .ok_or(ProgramError::InvalidInstructionData)?,
            )?,
            account_guid_hashes: unpack_account_guid_hash_vec(
                bytes
                    .get(GUID_HASH_BYTES..)
                    .ok_or(ProgramError::InvalidInstructionData)?,
            )?,
        })
    }

    fn unpack_finalize_balance_account_enable_spl_token(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::FinalizeBalanceAccountEnableSplToken {
            payer_account_guid_hash: unpack_account_guid_hash(
                bytes
                    .get(0..GUID_HASH_BYTES)
                    .ok_or(ProgramError::InvalidInstructionData)?,
            )?,
            account_guid_hashes: unpack_account_guid_hash_vec(
                bytes
                    .get(GUID_HASH_BYTES..)
                    .ok_or(ProgramError::InvalidInstructionData)?,
            )?,
        })
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct InitialWalletConfig {
    pub approvals_required_for_config: u8,
    pub approval_timeout_for_config: Duration,
    pub signers: Vec<(SlotId<Signer>, Signer)>,
    pub config_approvers: Vec<(SlotId<Signer>, Signer)>,
}

impl InitialWalletConfig {
    fn unpack(bytes: &[u8]) -> Result<InitialWalletConfig, ProgramError> {
        if bytes.len() < 7 {
            return Err(ProgramError::InvalidInstructionData);
        }
        let mut iter = bytes.iter();
        let approvals_required_for_config =
            *iter.next().ok_or(ProgramError::InvalidInstructionData)?;
        let approval_timeout_for_config =
            read_duration(&mut iter).ok_or(ProgramError::InvalidInstructionData)?;
        let signers = read_signers(&mut iter)?;
        let config_approvers = read_signers(&mut iter)?;

        Ok(InitialWalletConfig {
            approvals_required_for_config,
            approval_timeout_for_config,
            signers,
            config_approvers,
        })
    }

    pub fn pack(&self, dst: &mut Vec<u8>) {
        dst.push(self.approvals_required_for_config);
        append_duration(&self.approval_timeout_for_config, dst);
        append_signers(&self.signers, dst);
        append_signers(&self.config_approvers, dst);
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct BalanceAccountWhitelistUpdate {
    pub guid_hash: BalanceAccountGuidHash,
    pub add_allowed_destinations: Vec<(SlotId<AddressBookEntry>, AddressBookEntry)>,
    pub remove_allowed_destinations: Vec<(SlotId<AddressBookEntry>, AddressBookEntry)>,
}

impl BalanceAccountWhitelistUpdate {
    fn unpack_from_slice(
        iter: &mut Iter<u8>,
    ) -> Result<BalanceAccountWhitelistUpdate, ProgramError> {
        Ok(BalanceAccountWhitelistUpdate {
            guid_hash: unpack_account_guid_hash(
                read_slice(iter, GUID_HASH_BYTES).ok_or(ProgramError::InvalidInstructionData)?,
            )?,
            add_allowed_destinations: read_address_book_entries(iter)?,
            remove_allowed_destinations: read_address_book_entries(iter)?,
        })
    }

    pub fn pack_into_slice(&self, dst: &mut Vec<u8>) {
        dst.extend_from_slice(&self.guid_hash.to_bytes());
        append_address_book_entries(&self.add_allowed_destinations, dst);
        append_address_book_entries(&self.remove_allowed_destinations, dst);
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct AddressBookUpdate {
    pub add_address_book_entries: Vec<(SlotId<AddressBookEntry>, AddressBookEntry)>,
    pub remove_address_book_entries: Vec<(SlotId<AddressBookEntry>, AddressBookEntry)>,
    pub balance_account_whitelist_updates: Vec<BalanceAccountWhitelistUpdate>,
}

impl AddressBookUpdate {
    fn unpack(bytes: &[u8]) -> Result<AddressBookUpdate, ProgramError> {
        let mut iter = bytes.iter();

        let add_address_book_entries = read_address_book_entries(&mut iter)?;
        let remove_address_book_entries = read_address_book_entries(&mut iter)?;
        let balance_account_whitelist_updates = read_balance_account_whitelist_updates(&mut iter)?;

        Ok(AddressBookUpdate {
            add_address_book_entries,
            remove_address_book_entries,
            balance_account_whitelist_updates,
        })
    }

    pub fn pack(&self, dst: &mut Vec<u8>) {
        append_address_book_entries(&self.add_address_book_entries, dst);
        append_address_book_entries(&self.remove_address_book_entries, dst);
        append_balance_account_whitelist_updates(&self.balance_account_whitelist_updates, dst);
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct WalletConfigPolicyUpdate {
    pub approvals_required_for_config: Option<u8>,
    pub approval_timeout_for_config: Option<Duration>,
    pub add_config_approvers: Vec<(SlotId<Signer>, Signer)>,
    pub remove_config_approvers: Vec<(SlotId<Signer>, Signer)>,
}

impl WalletConfigPolicyUpdate {
    fn unpack(bytes: &[u8]) -> Result<WalletConfigPolicyUpdate, ProgramError> {
        let mut iter = bytes.iter();
        let approvals_required_for_config = read_optional_u8(&mut iter)?;
        let approval_timeout_for_config = read_optional_duration(&mut iter)?;
        let add_config_approvers = read_signers(&mut iter)?;
        let remove_config_approvers = read_signers(&mut iter)?;

        Ok(WalletConfigPolicyUpdate {
            approvals_required_for_config,
            approval_timeout_for_config,
            add_config_approvers,
            remove_config_approvers,
        })
    }

    pub fn pack(&self, dst: &mut Vec<u8>) {
        append_optional_u8(&self.approvals_required_for_config, dst);
        append_optional_duration(&self.approval_timeout_for_config, dst);
        append_signers(&self.add_config_approvers, dst);
        append_signers(&self.remove_config_approvers, dst);
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct BalanceAccountCreation {
    pub slot_id: SlotId<BalanceAccount>,
    pub name_hash: BalanceAccountNameHash,
    pub approvals_required_for_transfer: u8,
    pub approval_timeout_for_transfer: Duration,
    pub transfer_approvers: Vec<(SlotId<Signer>, Signer)>,
    pub whitelist_enabled: BooleanSetting,
    pub dapps_enabled: BooleanSetting,
    pub address_book_slot_id: SlotId<AddressBookEntry>,
}

impl BalanceAccountCreation {
    fn unpack(bytes: &[u8]) -> Result<BalanceAccountCreation, ProgramError> {
        if bytes.len() < 1 {
            return Err(ProgramError::InvalidInstructionData);
        }
        let mut iter = bytes.iter();
        let slot_id = *iter.next().ok_or(ProgramError::InvalidInstructionData)?;
        let name_hash: [u8; NAME_HASH_BYTES] =
            *read_fixed_size_array(&mut iter).ok_or(ProgramError::InvalidInstructionData)?;
        let approvals_required_for_transfer =
            *read_u8(&mut iter).ok_or(ProgramError::InvalidInstructionData)?;
        let approval_timeout_for_transfer =
            read_duration(&mut iter).ok_or(ProgramError::InvalidInstructionData)?;
        let transfer_approvers = read_signers(&mut iter)?;
        let whitelist_enabled = *iter.next().ok_or(ProgramError::InvalidInstructionData)?;
        let dapps_enabled = *iter.next().ok_or(ProgramError::InvalidInstructionData)?;
        let address_book_slot_id = *iter.next().ok_or(ProgramError::InvalidInstructionData)?;

        Ok(BalanceAccountCreation {
            slot_id: SlotId::new(slot_id as usize),
            name_hash: BalanceAccountNameHash::new(&name_hash),
            approvals_required_for_transfer,
            approval_timeout_for_transfer,
            transfer_approvers,
            whitelist_enabled: BooleanSetting::from_u8(whitelist_enabled),
            dapps_enabled: BooleanSetting::from_u8(dapps_enabled),
            address_book_slot_id: SlotId::new(address_book_slot_id as usize),
        })
    }

    pub fn pack(&self, dst: &mut Vec<u8>) {
        dst.push(self.slot_id.value as u8);
        dst.extend_from_slice(self.name_hash.to_bytes());
        dst.push(self.approvals_required_for_transfer);
        append_duration(&self.approval_timeout_for_transfer, dst);
        append_signers(&self.transfer_approvers, dst);
        dst.push(self.whitelist_enabled.to_u8());
        dst.push(self.dapps_enabled.to_u8());
        dst.push(self.address_book_slot_id.value as u8);
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct BalanceAccountPolicyUpdate {
    pub approvals_required_for_transfer: Option<u8>,
    pub approval_timeout_for_transfer: Option<Duration>,
    pub add_transfer_approvers: Vec<(SlotId<Signer>, Signer)>,
    pub remove_transfer_approvers: Vec<(SlotId<Signer>, Signer)>,
}

impl BalanceAccountPolicyUpdate {
    fn unpack(bytes: &[u8]) -> Result<BalanceAccountPolicyUpdate, ProgramError> {
        if bytes.len() < 1 {
            return Err(ProgramError::InvalidInstructionData);
        }
        let mut iter = bytes.iter();
        let approvals_required_for_transfer = read_optional_u8(&mut iter)?;
        let approval_timeout_for_transfer = read_optional_duration(&mut iter)?;
        let add_approvers = read_signers(&mut iter)?;
        let remove_approvers = read_signers(&mut iter)?;

        Ok(BalanceAccountPolicyUpdate {
            approvals_required_for_transfer,
            approval_timeout_for_transfer,
            add_transfer_approvers: add_approvers,
            remove_transfer_approvers: remove_approvers,
        })
    }

    pub fn pack(&self, dst: &mut Vec<u8>) {
        append_optional_u8(&self.approvals_required_for_transfer, dst);
        append_optional_duration(&self.approval_timeout_for_transfer, dst);
        append_signers(&self.add_transfer_approvers, dst);
        append_signers(&self.remove_transfer_approvers, dst);
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct DAppBookUpdate {
    pub add_dapps: Vec<(SlotId<DAppBookEntry>, DAppBookEntry)>,
    pub remove_dapps: Vec<(SlotId<DAppBookEntry>, DAppBookEntry)>,
}

impl DAppBookUpdate {
    fn unpack(bytes: &[u8]) -> Result<DAppBookUpdate, ProgramError> {
        if bytes.len() < 1 {
            return Err(ProgramError::InvalidInstructionData);
        }
        let mut iter = bytes.iter();
        let add_dapps = read_address_book_entries(&mut iter)?;
        let remove_dapps = read_address_book_entries(&mut iter)?;

        Ok(DAppBookUpdate {
            add_dapps,
            remove_dapps,
        })
    }

    pub fn pack(&self, dst: &mut Vec<u8>) {
        append_address_book_entries(&self.add_dapps, dst);
        append_address_book_entries(&self.remove_dapps, dst);
    }
}

fn read_signers(iter: &mut Iter<u8>) -> Result<Vec<(SlotId<Signer>, Signer)>, ProgramError> {
    let signers_count = *read_u8(iter).ok_or(ProgramError::InvalidInstructionData)?;
    read_slice(iter, usize::from(signers_count) * (1 + Signer::LEN))
        .ok_or(ProgramError::InvalidInstructionData)?
        .chunks_exact(1 + Signer::LEN)
        .map(|chunk| {
            Signer::unpack_from_slice(&chunk[1..1 + Signer::LEN])
                .map(|signer| (SlotId::new(usize::from(chunk[0])), signer))
        })
        .collect()
}

fn append_signers(signers: &Vec<(SlotId<Signer>, Signer)>, dst: &mut Vec<u8>) {
    dst.push(signers.len() as u8);
    for (slot_id, signer) in signers.iter() {
        let mut buf = vec![0; 1 + Signer::LEN];
        buf[0] = slot_id.value as u8;
        signer.pack_into_slice(&mut buf[1..1 + Signer::LEN]);
        dst.extend_from_slice(buf.as_slice());
    }
}

fn read_account_guid_vec(iter: &mut Iter<u8>) -> Result<Vec<BalanceAccountGuidHash>, ProgramError> {
    let n = *read_u8(iter).ok_or(ProgramError::InvalidInstructionData)?;
    Ok((0..n).map(|_| read_account_guid(iter).unwrap()).collect())
}

fn read_account_guid(iter: &mut Iter<u8>) -> Result<BalanceAccountGuidHash, ProgramError> {
    unpack_account_guid_hash(
        read_slice(iter, GUID_HASH_BYTES)
            .ok_or(ProgramError::InvalidInstructionData)?
            .into(),
    )
}

fn read_instructions(iter: &mut Iter<u8>) -> Result<Vec<Instruction>, ProgramError> {
    let account_meta_count = read_u8(iter).ok_or(ProgramError::InvalidInstructionData)?;
    let account_metas = (0..*account_meta_count)
        .map(|_| read_account_meta(iter).unwrap())
        .collect();

    let instruction_count = read_u16(iter).ok_or(ProgramError::InvalidInstructionData)?;
    Ok((0..instruction_count)
        .map(|_| read_instruction(iter, &account_metas).unwrap())
        .collect())
}

fn read_account_meta(iter: &mut Iter<u8>) -> Result<AccountMeta, ProgramError> {
    let flags = *read_u8(iter)
        .ok_or(ProgramError::InvalidInstructionData)
        .unwrap();
    let pubkey = Pubkey::new(
        read_slice(iter, PUBKEY_BYTES)
            .ok_or(ProgramError::InvalidInstructionData)
            .unwrap()
            .try_into()
            .ok()
            .unwrap(),
    );
    Ok(AccountMeta {
        is_writable: (flags & 1) == 1,
        is_signer: (flags & 2) == 2,
        pubkey,
    })
}

fn read_instruction(
    iter: &mut Iter<u8>,
    account_metas: &Vec<AccountMeta>,
) -> Result<Instruction, ProgramError> {
    let program_id_index = *read_u8(iter)
        .ok_or(ProgramError::InvalidInstructionData)
        .unwrap();
    let program_id = account_metas[program_id_index.as_usize()].pubkey;
    let account_count = read_u16(iter).ok_or(ProgramError::InvalidInstructionData)?;
    let accounts = (0..account_count)
        .map(|_| {
            let index = *read_u8(iter)
                .ok_or(ProgramError::InvalidInstructionData)
                .unwrap();
            account_metas[index.as_usize()].clone()
        })
        .collect();
    let data_len = read_u16(iter).ok_or(ProgramError::InvalidInstructionData)?;
    let data = read_slice(iter, data_len.try_into().unwrap())
        .ok_or(ProgramError::InvalidInstructionData)?
        .to_vec();
    Ok(Instruction {
        program_id,
        accounts,
        data,
    })
}

fn index_of_account(account_metas: &Vec<AccountMeta>, pubkey: &Pubkey) -> u8 {
    account_metas
        .iter()
        .position(|a| a.pubkey == *pubkey)
        .unwrap()
        .as_u8()
}

pub fn append_instruction(
    instruction: &Instruction,
    account_metas: &Vec<AccountMeta>,
    dst: &mut Vec<u8>,
) {
    dst.put_u8(index_of_account(account_metas, &instruction.program_id));
    dst.put_u16_le(instruction.accounts.len() as u16);
    for account in instruction.accounts.iter() {
        dst.put_u8(index_of_account(account_metas, &account.pubkey).as_u8());
    }
    dst.put_u16_le(instruction.data.len().as_u16());
    dst.extend_from_slice(instruction.data.as_slice());
}

pub fn append_instruction_expanded(instruction: &Instruction, dst: &mut Vec<u8>) {
    dst.extend_from_slice(instruction.program_id.as_ref());
    dst.put_u16_le(instruction.accounts.len() as u16);
    for account in instruction.accounts.iter() {
        let mut buf = vec![0; 1 + Signer::LEN];
        buf[0] = 0;
        if account.is_signer {
            buf[0] |= 2;
        }
        if account.is_writable {
            buf[0] |= 1;
        }
        Signer::new(account.pubkey).pack_into_slice(&mut buf[1..1 + Signer::LEN]);
        dst.extend_from_slice(buf.as_slice());
    }
    dst.put_u16_le(instruction.data.len().as_u16());
    dst.extend_from_slice(instruction.data.as_slice());
}

fn read_address_book_entries(
    iter: &mut Iter<u8>,
) -> Result<Vec<(SlotId<AddressBookEntry>, AddressBookEntry)>, ProgramError> {
    let entries_count = *read_u8(iter).ok_or(ProgramError::InvalidInstructionData)?;
    read_slice(
        iter,
        usize::from(entries_count) * (1 + AddressBookEntry::LEN),
    )
    .ok_or(ProgramError::InvalidInstructionData)?
    .chunks_exact(1 + AddressBookEntry::LEN)
    .map(|chunk| {
        AddressBookEntry::unpack_from_slice(&chunk[1..1 + AddressBookEntry::LEN])
            .map(|entry| (SlotId::new(usize::from(chunk[0])), entry))
    })
    .collect()
}

fn append_address_book_entries(
    entries: &Vec<(SlotId<AddressBookEntry>, AddressBookEntry)>,
    dst: &mut Vec<u8>,
) {
    dst.push(entries.len() as u8);
    for (slot_id, entry) in entries.iter() {
        let mut buf = vec![0; 1 + AddressBookEntry::LEN];
        buf[0] = slot_id.value as u8;
        entry.pack_into_slice(&mut buf[1..1 + AddressBookEntry::LEN]);
        dst.extend_from_slice(buf.as_slice());
    }
}

pub fn unpack_account_guid_hash_vec(
    bytes: &[u8],
) -> Result<Vec<BalanceAccountGuidHash>, ProgramError> {
    let iter = &mut bytes.iter();
    read_account_guid_vec(iter)
}

fn unpack_account_guid_hash(bytes: &[u8]) -> Result<BalanceAccountGuidHash, ProgramError> {
    bytes
        .get(..GUID_HASH_BYTES)
        .and_then(|slice| {
            slice
                .try_into()
                .ok()
                .map(|bytes| BalanceAccountGuidHash::new(bytes))
        })
        .ok_or(ProgramError::InvalidInstructionData)
}

fn unpack_account_name_hash(bytes: &[u8]) -> Result<BalanceAccountNameHash, ProgramError> {
    bytes
        .get(..NAME_HASH_BYTES)
        .and_then(|slice| {
            slice
                .try_into()
                .ok()
                .map(|bytes| BalanceAccountNameHash::new(bytes))
        })
        .ok_or(ProgramError::InvalidInstructionData)
}

/// Deserialize a Pubkey, starting from the given offset in `bytes` slice.
fn unpack_public_key(bytes: &[u8], offset: usize) -> Result<Pubkey, ProgramError> {
    Ok(Pubkey::new_from_array(
        bytes
            .get(offset..offset + PUBKEY_BYTES)
            .and_then(|slice| slice.try_into().ok())
            .ok_or(ProgramError::InvalidInstructionData)?,
    ))
}

fn append_balance_account_whitelist_updates(
    entries: &Vec<BalanceAccountWhitelistUpdate>,
    dst: &mut Vec<u8>,
) {
    dst.push(entries.len() as u8);
    for entry in entries.iter() {
        entry.pack_into_slice(dst);
    }
}

fn read_balance_account_whitelist_updates(
    iter: &mut Iter<u8>,
) -> Result<Vec<BalanceAccountWhitelistUpdate>, ProgramError> {
    let entries_count = *read_u8(iter).ok_or(ProgramError::InvalidInstructionData)? as usize;
    let mut updates: Vec<BalanceAccountWhitelistUpdate> = Vec::with_capacity(entries_count);
    for _ in 0..entries_count {
        updates.push(BalanceAccountWhitelistUpdate::unpack_from_slice(iter)?)
    }
    Ok(updates)
}

pub fn pack_balance_account_guid_hash_vec(hashes: &Vec<BalanceAccountGuidHash>, buf: &mut Vec<u8>) {
    buf.put_u8(hashes.len() as u8);
    for h in hashes.iter() {
        buf.extend_from_slice(h.to_bytes());
    }
}
