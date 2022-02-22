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

use crate::model::address_book::{AddressBookEntry, AddressBookEntryNameHash, DAppBookEntry};
use crate::model::balance_account::{BalanceAccountGuidHash, BalanceAccountNameHash};
use crate::model::multisig_op::{
    ApprovalDisposition, BooleanSetting, SlotUpdateType, WrapDirection,
};
use crate::model::signer::Signer;
use crate::utils;
use crate::utils::{pack_option, SlotId};

#[derive(Debug)]
pub enum ProgramInstruction {
    /// 0. `[writable]` The wallet account
    /// 1. `[signer]` The transaction assistant account
    InitWallet { update: WalletUpdate },

    /// 0. `[writable]` The multisig operation account
    /// 1. `[]` The wallet account
    /// 2. `[signer]` The initiator account (either the transaction assistant or an approver)
    /// 3. `[]` The sysvar clock account
    InitWalletUpdate { update: WalletUpdate },

    /// 0. `[writable]` The multisig operation account
    /// 1. `[writable]` The wallet account
    /// 2. `[signer]` The rent collector account
    FinalizeWalletUpdate { update: WalletUpdate },

    /// 0. `[writable]` The multisig operation account
    /// 1. `[]` The wallet account
    /// 2. `[signer]` The initiator account (either the transaction assistant or an approver)
    /// 3. `[]` The sysvar clock account
    InitBalanceAccountCreation {
        account_guid_hash: BalanceAccountGuidHash,
        update: BalanceAccountUpdate,
    },

    /// 0. `[writable]` The multisig operation account
    /// 1. `[writable]` The wallet account
    /// 2. `[signer]` The rent collector account
    FinalizeBalanceAccountCreation {
        account_guid_hash: BalanceAccountGuidHash,
        update: BalanceAccountUpdate,
    },

    /// 0. `[writable]` The multisig operation account
    /// 1. `[]` The wallet account
    /// 2. `[signer]` The initiator account (either the transaction assistant or an approver)
    /// 3. `[]` The sysvar clock account
    InitBalanceAccountUpdate {
        account_guid_hash: BalanceAccountGuidHash,
        update: BalanceAccountUpdate,
    },

    /// 0. `[writable]` The multisig operation account
    /// 1. `[writable]` The wallet account
    /// 2. `[signer]` The rent collector account
    /// 3. `[]` The sysvar clock account
    FinalizeBalanceAccountUpdate {
        account_guid_hash: BalanceAccountGuidHash,
        update: BalanceAccountUpdate,
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

    InitBalanceAccountNameHashUpdate {
        account_guid_hash: BalanceAccountGuidHash,
        account_name_hash: BalanceAccountNameHash,
    },

    FinalizeBalanceAccountNameHashUpdate {
        account_guid_hash: BalanceAccountGuidHash,
        account_name_hash: BalanceAccountNameHash,
    },
}

impl ProgramInstruction {
    pub fn pack(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(size_of::<Self>());
        match self {
            &ProgramInstruction::InitWallet { ref update } => {
                let mut update_bytes: Vec<u8> = Vec::new();
                update.pack(&mut update_bytes);
                buf.push(0);
                buf.extend_from_slice(&update_bytes);
            }
            &ProgramInstruction::InitWalletUpdate { ref update } => {
                let mut update_bytes: Vec<u8> = Vec::new();
                update.pack(&mut update_bytes);
                buf.push(1);
                buf.extend_from_slice(&update_bytes);
            }
            &ProgramInstruction::FinalizeWalletUpdate { ref update } => {
                let mut update_bytes: Vec<u8> = Vec::new();
                update.pack(&mut update_bytes);
                buf.push(2);
                buf.extend_from_slice(&update_bytes);
            }
            &ProgramInstruction::SetApprovalDisposition {
                ref disposition,
                ref params_hash,
            } => {
                buf.push(9);
                buf.push(disposition.to_u8());
                buf.extend_from_slice(params_hash.as_ref());
            }
            &ProgramInstruction::InitBalanceAccountCreation {
                ref account_guid_hash,
                ref update,
            } => {
                let mut update_bytes: Vec<u8> = Vec::new();
                update.pack(&mut update_bytes);
                buf.push(3);
                buf.extend_from_slice(account_guid_hash.to_bytes());
                buf.extend_from_slice(&update_bytes);
            }
            &ProgramInstruction::FinalizeBalanceAccountCreation {
                ref account_guid_hash,
                ref update,
            } => {
                let mut update_bytes: Vec<u8> = Vec::new();
                update.pack(&mut update_bytes);
                buf.push(4);
                buf.extend_from_slice(account_guid_hash.to_bytes());
                buf.extend_from_slice(&update_bytes);
            }
            &ProgramInstruction::InitBalanceAccountUpdate {
                ref account_guid_hash,
                ref update,
            } => {
                let mut update_bytes: Vec<u8> = Vec::new();
                update.pack(&mut update_bytes);
                buf.push(5);
                buf.extend_from_slice(account_guid_hash.to_bytes());
                buf.extend_from_slice(&update_bytes);
            }
            &ProgramInstruction::FinalizeBalanceAccountUpdate {
                ref account_guid_hash,
                ref update,
            } => {
                let mut update_bytes: Vec<u8> = Vec::new();
                update.pack(&mut update_bytes);
                buf.push(6);
                buf.extend_from_slice(account_guid_hash.to_bytes());
                buf.extend_from_slice(&update_bytes);
            }
            &ProgramInstruction::InitTransfer {
                ref account_guid_hash,
                ref amount,
                ref destination_name_hash,
            } => {
                buf.push(7);
                buf.extend_from_slice(account_guid_hash.to_bytes());
                buf.extend_from_slice(&amount.to_le_bytes());
                buf.extend_from_slice(destination_name_hash.to_bytes());
            }
            &ProgramInstruction::FinalizeTransfer {
                ref account_guid_hash,
                ref amount,
                ref token_mint,
            } => {
                buf.push(8);
                buf.extend_from_slice(account_guid_hash.to_bytes());
                buf.extend_from_slice(&amount.to_le_bytes());
                buf.extend_from_slice(&token_mint.to_bytes());
                buf.push(0);
            }
            &ProgramInstruction::InitWrapUnwrap {
                ref account_guid_hash,
                ref amount,
                ref direction,
            } => {
                buf.push(10);
                buf.extend_from_slice(&account_guid_hash.to_bytes());
                buf.extend_from_slice(&amount.to_le_bytes());
                buf.push(direction.to_u8());
            }
            &ProgramInstruction::FinalizeWrapUnwrap {
                ref account_guid_hash,
                ref amount,
                ref direction,
            } => {
                buf.push(11);
                buf.extend_from_slice(&account_guid_hash.to_bytes());
                buf.extend_from_slice(&amount.to_le_bytes());
                buf.push(direction.to_u8());
            }
            &ProgramInstruction::InitUpdateSigner {
                ref slot_update_type,
                ref slot_id,
                ref signer,
            } => {
                buf.push(12);
                buf.push(slot_update_type.to_u8());
                buf.push(slot_id.value as u8);
                buf.extend_from_slice(signer.key.as_ref());
            }
            &ProgramInstruction::FinalizeUpdateSigner {
                ref slot_update_type,
                ref slot_id,
                ref signer,
            } => {
                buf.push(13);
                buf.push(slot_update_type.to_u8());
                buf.push(slot_id.value as u8);
                buf.extend_from_slice(signer.key.as_ref());
            }
            &ProgramInstruction::InitWalletConfigPolicyUpdate { ref update } => {
                let mut update_bytes: Vec<u8> = Vec::new();
                update.pack(&mut update_bytes);
                buf.push(14);
                buf.extend_from_slice(&update_bytes);
            }
            &ProgramInstruction::FinalizeWalletConfigPolicyUpdate { ref update } => {
                let mut update_bytes: Vec<u8> = Vec::new();
                update.pack(&mut update_bytes);
                buf.push(15);
                buf.extend_from_slice(&update_bytes);
            }
            &ProgramInstruction::InitDAppTransaction {
                ref account_guid_hash,
                ref dapp,
                ref instructions,
            } => {
                buf.push(16);
                buf.extend_from_slice(&account_guid_hash.to_bytes());
                let mut buf2 = vec![0; DAppBookEntry::LEN];
                dapp.pack_into_slice(buf2.as_mut_slice());
                buf.extend_from_slice(&buf2[..]);
                buf.put_u16_le(instructions.len() as u16);
                for instruction in instructions.iter() {
                    append_instruction(instruction, &mut buf);
                }
            }
            &ProgramInstruction::FinalizeDAppTransaction {
                ref account_guid_hash,
                ref dapp,
                ref instructions,
            } => {
                buf.push(17);
                buf.extend_from_slice(&account_guid_hash.to_bytes());
                let mut buf2 = vec![0; DAppBookEntry::LEN];
                dapp.pack_into_slice(buf2.as_mut_slice());
                buf.extend_from_slice(&buf2[..]);
                buf.put_u16_le(instructions.len() as u16);
                for instruction in instructions.iter() {
                    append_instruction(instruction, &mut buf);
                }
            }
            &ProgramInstruction::InitAccountSettingsUpdate {
                ref account_guid_hash,
                ref whitelist_enabled,
                ref dapps_enabled,
            } => {
                buf.push(18);
                buf.extend_from_slice(&account_guid_hash.to_bytes());
                pack_option(whitelist_enabled.as_ref(), &mut buf);
                pack_option(dapps_enabled.as_ref(), &mut buf);
            }
            &ProgramInstruction::FinalizeAccountSettingsUpdate {
                ref account_guid_hash,
                ref whitelist_enabled,
                ref dapps_enabled,
            } => {
                buf.push(19);
                buf.extend_from_slice(&account_guid_hash.to_bytes());
                pack_option(whitelist_enabled.as_ref(), &mut buf);
                pack_option(dapps_enabled.as_ref(), &mut buf);
            }
            &ProgramInstruction::InitDAppBookUpdate { ref update } => {
                buf.push(20);
                let mut update_bytes: Vec<u8> = Vec::new();
                update.pack(&mut update_bytes);
                buf.extend_from_slice(&update_bytes);
            }
            &ProgramInstruction::FinalizeDAppBookUpdate { ref update } => {
                buf.push(21);
                let mut update_bytes: Vec<u8> = Vec::new();
                update.pack(&mut update_bytes);
                buf.extend_from_slice(&update_bytes);
            }
            &ProgramInstruction::InitBalanceAccountNameHashUpdate {
                ref account_guid_hash,
                ref account_name_hash,
            } => {
                buf.push(22);
                buf.extend_from_slice(account_guid_hash.to_bytes());
                buf.extend_from_slice(account_name_hash.to_bytes());
            }
            &ProgramInstruction::FinalizeBalanceAccountNameHashUpdate {
                ref account_guid_hash,
                ref account_name_hash,
            } => {
                buf.push(23);
                buf.extend_from_slice(account_guid_hash.to_bytes());
                buf.extend_from_slice(account_name_hash.to_bytes());
            }
        }
        buf
    }

    pub fn unpack(input: &[u8]) -> Result<Self, ProgramError> {
        let (tag, rest) = input
            .split_first()
            .ok_or(ProgramError::InvalidInstructionData)?;
        Ok(match tag {
            0 => Self::unpack_init_wallet_instruction(rest)?,
            1 => Self::unpack_init_wallet_update_instruction(rest)?,
            2 => Self::unpack_finalize_wallet_update_instruction(rest)?,
            3 => Self::unpack_init_balance_account_creation_instruction(rest)?,
            4 => Self::unpack_finalize_balance_account_creation_instruction(rest)?,
            5 => Self::unpack_init_balance_account_update_instruction(rest)?,
            6 => Self::unpack_finalize_balance_account_update_instruction(rest)?,
            7 => Self::unpack_init_transfer_for_approval_instruction(rest)?,
            8 => Self::unpack_finalize_transfer_instruction(rest)?,
            9 => Self::unpack_set_approval_disposition_instruction(rest)?,
            10 => Self::unpack_init_wrap_unwrap_instruction(rest)?,
            11 => Self::unpack_finalize_wrap_unwrap_instruction(rest)?,
            12 => Self::unpack_init_update_signer_instruction(rest)?,
            13 => Self::unpack_finalize_update_signer_instruction(rest)?,
            14 => Self::unpack_init_wallet_config_policy_update_instruction(rest)?,
            15 => Self::unpack_finalize_wallet_config_policy_update_instruction(rest)?,
            16 => Self::unpack_init_dapp_transaction_instruction(rest)?,
            17 => Self::unpack_finalize_dapp_transaction_instruction(rest)?,
            18 => Self::unpack_init_account_settings_update_instruction(rest)?,
            19 => Self::unpack_finalize_account_settings_update_instruction(rest)?,
            20 => Self::unpack_init_dapp_book_update_instruction(rest)?,
            21 => Self::unpack_finalize_dapp_book_update_instruction(rest)?,
            22 => Self::unpack_init_balance_account_name_hash_update_instruction(rest)?,
            23 => Self::unpack_finalize_balance_account_name_hash_update_instruction(rest)?,
            _ => return Err(ProgramError::InvalidInstructionData),
        })
    }

    fn unpack_init_wallet_instruction(bytes: &[u8]) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::InitWallet {
            update: WalletUpdate::unpack(bytes)?,
        })
    }

    fn unpack_init_wallet_update_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::InitWalletUpdate {
            update: WalletUpdate::unpack(bytes)?,
        })
    }

    fn unpack_finalize_wallet_update_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::FinalizeWalletUpdate {
            update: WalletUpdate::unpack(bytes)?,
        })
    }

    fn unpack_init_balance_account_creation_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::InitBalanceAccountCreation {
            account_guid_hash: unpack_account_guid_hash(bytes)?,
            update: BalanceAccountUpdate::unpack(
                bytes
                    .get(32..)
                    .ok_or(ProgramError::InvalidInstructionData)?,
            )?,
        })
    }

    fn unpack_finalize_balance_account_creation_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::FinalizeBalanceAccountCreation {
            account_guid_hash: unpack_account_guid_hash(bytes)?,
            update: BalanceAccountUpdate::unpack(
                bytes
                    .get(32..)
                    .ok_or(ProgramError::InvalidInstructionData)?,
            )?,
        })
    }

    fn unpack_init_balance_account_update_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::InitBalanceAccountUpdate {
            account_guid_hash: unpack_account_guid_hash(bytes)?,
            update: BalanceAccountUpdate::unpack(
                bytes
                    .get(32..)
                    .ok_or(ProgramError::InvalidInstructionData)?,
            )?,
        })
    }

    fn unpack_finalize_balance_account_update_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::FinalizeBalanceAccountUpdate {
            account_guid_hash: unpack_account_guid_hash(bytes)?,
            update: BalanceAccountUpdate::unpack(
                bytes
                    .get(32..)
                    .ok_or(ProgramError::InvalidInstructionData)?,
            )?,
        })
    }

    fn unpack_init_transfer_for_approval_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        let account_guid_hash = unpack_account_guid_hash(bytes)?;

        let amount = bytes
            .get(32..40)
            .and_then(|slice| slice.try_into().ok())
            .map(u64::from_le_bytes)
            .ok_or(ProgramError::InvalidInstructionData)?;

        let destination_name_hash = bytes
            .get(40..72)
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
                rest.get(0..32)
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
                .get(32..40)
                .and_then(|slice| slice.try_into().ok())
                .map(u64::from_le_bytes)
                .ok_or(ProgramError::InvalidInstructionData)?,
            token_mint: Pubkey::new_from_array(
                bytes
                    .get(40..72)
                    .and_then(|slice| slice.try_into().ok())
                    .ok_or(ProgramError::InvalidInstructionData)?,
            ),
        })
    }

    fn unpack_init_wrap_unwrap_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        if let Some(direction) = bytes.get(40) {
            Ok(Self::InitWrapUnwrap {
                account_guid_hash: unpack_account_guid_hash(bytes)?,
                amount: bytes
                    .get(32..40)
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
                    .get(32..40)
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
            utils::read_slice(iter, 32).ok_or(ProgramError::InvalidInstructionData)?,
        )?;
        let dapp = DAppBookEntry::unpack_from_slice(
            utils::read_slice(iter, DAppBookEntry::LEN)
                .ok_or(ProgramError::InvalidInstructionData)?,
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
            utils::read_slice(iter, 32).ok_or(ProgramError::InvalidInstructionData)?,
        )?;
        let dapp = DAppBookEntry::unpack_from_slice(
            utils::read_slice(iter, DAppBookEntry::LEN)
                .ok_or(ProgramError::InvalidInstructionData)?,
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
                utils::read_slice(iter, 32).ok_or(ProgramError::InvalidInstructionData)?,
            )?,
            whitelist_enabled: utils::unpack_option::<BooleanSetting>(iter)?,
            dapps_enabled: utils::unpack_option::<BooleanSetting>(iter)?,
        })
    }

    fn unpack_finalize_account_settings_update_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        let iter = &mut bytes.into_iter();
        Ok(Self::FinalizeAccountSettingsUpdate {
            account_guid_hash: unpack_account_guid_hash(
                utils::read_slice(iter, 32).ok_or(ProgramError::InvalidInstructionData)?,
            )?,
            whitelist_enabled: utils::unpack_option::<BooleanSetting>(iter)?,
            dapps_enabled: utils::unpack_option::<BooleanSetting>(iter)?,
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

    fn unpack_init_balance_account_name_hash_update_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::InitBalanceAccountNameHashUpdate {
            account_guid_hash: unpack_account_guid_hash(bytes)?,
            account_name_hash: unpack_account_name_hash(
                bytes
                    .get(32..)
                    .ok_or(ProgramError::InvalidInstructionData)?,
            )?,
        })
    }

    fn unpack_finalize_balance_account_name_hash_update_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::FinalizeBalanceAccountNameHashUpdate {
            account_guid_hash: unpack_account_guid_hash(bytes)?,
            account_name_hash: unpack_account_name_hash(
                bytes
                    .get(32..)
                    .ok_or(ProgramError::InvalidInstructionData)?,
            )?,
        })
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct WalletUpdate {
    pub approvals_required_for_config: u8,
    pub approval_timeout_for_config: Duration,
    pub add_signers: Vec<(SlotId<Signer>, Signer)>,
    pub remove_signers: Vec<(SlotId<Signer>, Signer)>,
    pub add_config_approvers: Vec<(SlotId<Signer>, Signer)>,
    pub remove_config_approvers: Vec<(SlotId<Signer>, Signer)>,
    pub add_address_book_entries: Vec<(SlotId<AddressBookEntry>, AddressBookEntry)>,
    pub remove_address_book_entries: Vec<(SlotId<AddressBookEntry>, AddressBookEntry)>,
}

impl WalletUpdate {
    fn unpack(bytes: &[u8]) -> Result<WalletUpdate, ProgramError> {
        if bytes.len() < 7 {
            return Err(ProgramError::InvalidInstructionData);
        }
        let mut iter = bytes.iter();
        let approvals_required_for_config =
            *iter.next().ok_or(ProgramError::InvalidInstructionData)?;
        let approval_timeout_for_config =
            read_duration(&mut iter).ok_or(ProgramError::InvalidInstructionData)?;
        let add_signers = read_signers(&mut iter)?;
        let remove_signers = read_signers(&mut iter)?;
        let add_config_approvers = read_signers(&mut iter)?;
        let remove_config_approvers = read_signers(&mut iter)?;
        let add_address_book_entries = read_address_book_entries(&mut iter)?;
        let remove_address_book_entries = read_address_book_entries(&mut iter)?;

        Ok(WalletUpdate {
            approvals_required_for_config,
            approval_timeout_for_config,
            add_signers,
            remove_signers,
            add_config_approvers,
            remove_config_approvers,
            add_address_book_entries,
            remove_address_book_entries,
        })
    }

    pub fn pack(&self, dst: &mut Vec<u8>) {
        dst.push(self.approvals_required_for_config);
        append_duration(&self.approval_timeout_for_config, dst);
        append_signers(&self.add_signers, dst);
        append_signers(&self.remove_signers, dst);
        append_signers(&self.add_config_approvers, dst);
        append_signers(&self.remove_config_approvers, dst);
        append_address_book_entries(&self.add_address_book_entries, dst);
        append_address_book_entries(&self.remove_address_book_entries, dst);
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct WalletConfigPolicyUpdate {
    pub approvals_required_for_config: u8,
    pub approval_timeout_for_config: Duration,
    pub add_config_approvers: Vec<(SlotId<Signer>, Signer)>,
    pub remove_config_approvers: Vec<(SlotId<Signer>, Signer)>,
}

impl WalletConfigPolicyUpdate {
    fn unpack(bytes: &[u8]) -> Result<WalletConfigPolicyUpdate, ProgramError> {
        let mut iter = bytes.iter();
        let approvals_required_for_config =
            *iter.next().ok_or(ProgramError::InvalidInstructionData)?;
        let approval_timeout_for_config =
            read_duration(&mut iter).ok_or(ProgramError::InvalidInstructionData)?;
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
        dst.push(self.approvals_required_for_config);
        append_duration(&self.approval_timeout_for_config, dst);
        append_signers(&self.add_config_approvers, dst);
        append_signers(&self.remove_config_approvers, dst);
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct BalanceAccountUpdate {
    pub name_hash: BalanceAccountNameHash,
    pub approvals_required_for_transfer: u8,
    pub approval_timeout_for_transfer: Duration,
    pub add_transfer_approvers: Vec<(SlotId<Signer>, Signer)>,
    pub remove_transfer_approvers: Vec<(SlotId<Signer>, Signer)>,
    pub add_allowed_destinations: Vec<(SlotId<AddressBookEntry>, AddressBookEntry)>,
    pub remove_allowed_destinations: Vec<(SlotId<AddressBookEntry>, AddressBookEntry)>,
}

impl BalanceAccountUpdate {
    fn unpack(bytes: &[u8]) -> Result<BalanceAccountUpdate, ProgramError> {
        if bytes.len() < 1 {
            return Err(ProgramError::InvalidInstructionData);
        }
        let mut iter = bytes.iter();
        let name_hash: [u8; 32] =
            *read_fixed_size_array(&mut iter).ok_or(ProgramError::InvalidInstructionData)?;
        let approvals_required_for_transfer =
            *read_u8(&mut iter).ok_or(ProgramError::InvalidInstructionData)?;
        let approval_timeout_for_transfer =
            read_duration(&mut iter).ok_or(ProgramError::InvalidInstructionData)?;
        let add_approvers = read_signers(&mut iter)?;
        let remove_approvers = read_signers(&mut iter)?;
        let add_allowed_destinations = read_address_book_entries(&mut iter)?;
        let remove_allowed_destinations = read_address_book_entries(&mut iter)?;

        Ok(BalanceAccountUpdate {
            name_hash: BalanceAccountNameHash::new(&name_hash),
            approvals_required_for_transfer,
            approval_timeout_for_transfer,
            add_transfer_approvers: add_approvers,
            remove_transfer_approvers: remove_approvers,
            add_allowed_destinations,
            remove_allowed_destinations,
        })
    }

    pub fn pack(&self, dst: &mut Vec<u8>) {
        dst.extend_from_slice(&self.name_hash.to_bytes());
        dst.push(self.approvals_required_for_transfer);
        append_duration(&self.approval_timeout_for_transfer, dst);
        append_signers(&self.add_transfer_approvers, dst);
        append_signers(&self.remove_transfer_approvers, dst);
        append_address_book_entries(&self.add_allowed_destinations, dst);
        append_address_book_entries(&self.remove_allowed_destinations, dst);
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

fn read_u8<'a>(iter: &'a mut Iter<u8>) -> Option<&'a u8> {
    iter.next()
}

fn read_u16(iter: &mut Iter<u8>) -> Option<u16> {
    read_fixed_size_array::<2>(iter).map(|slice| u16::from_le_bytes(*slice))
}

fn read_fixed_size_array<'a, const SIZE: usize>(iter: &'a mut Iter<u8>) -> Option<&'a [u8; SIZE]> {
    utils::read_slice(iter, SIZE).and_then(|slice| slice.try_into().ok())
}

fn read_duration(iter: &mut Iter<u8>) -> Option<Duration> {
    read_fixed_size_array::<8>(iter).map(|slice| Duration::from_secs(u64::from_le_bytes(*slice)))
}

fn append_duration(duration: &Duration, dst: &mut Vec<u8>) {
    dst.extend_from_slice(&duration.as_secs().to_le_bytes()[..])
}

fn read_signers(iter: &mut Iter<u8>) -> Result<Vec<(SlotId<Signer>, Signer)>, ProgramError> {
    let signers_count = *read_u8(iter).ok_or(ProgramError::InvalidInstructionData)?;
    utils::read_slice(iter, usize::from(signers_count) * (1 + Signer::LEN))
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

fn read_instructions(iter: &mut Iter<u8>) -> Result<Vec<Instruction>, ProgramError> {
    let instruction_count = read_u16(iter).ok_or(ProgramError::InvalidInstructionData)?;
    Ok((0..instruction_count)
        .map(|_| read_instruction(iter).unwrap())
        .collect())
}

fn read_instruction(iter: &mut Iter<u8>) -> Result<Instruction, ProgramError> {
    let program_id = Pubkey::new(
        utils::read_slice(iter, 32)
            .ok_or(ProgramError::InvalidInstructionData)?
            .into(),
    );
    let account_count = read_u16(iter).ok_or(ProgramError::InvalidInstructionData)?;
    let accounts = (0..account_count)
        .map(|_| {
            let flags = *read_u8(iter)
                .ok_or(ProgramError::InvalidInstructionData)
                .unwrap();
            let pubkey = Pubkey::new(
                utils::read_slice(iter, 32)
                    .ok_or(ProgramError::InvalidInstructionData)
                    .unwrap()
                    .try_into()
                    .ok()
                    .unwrap(),
            );
            AccountMeta {
                is_writable: (flags & 1) == 1,
                is_signer: (flags & 2) == 2,
                pubkey,
            }
        })
        .collect();
    let data_len = read_u16(iter).ok_or(ProgramError::InvalidInstructionData)?;
    let data = utils::read_slice(iter, data_len.try_into().unwrap())
        .ok_or(ProgramError::InvalidInstructionData)?
        .to_vec();
    Ok(Instruction {
        program_id,
        accounts,
        data,
    })
}

pub fn append_instruction(instruction: &Instruction, dst: &mut Vec<u8>) {
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
    utils::read_slice(
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

fn unpack_account_guid_hash(bytes: &[u8]) -> Result<BalanceAccountGuidHash, ProgramError> {
    bytes
        .get(..32)
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
        .get(..32)
        .and_then(|slice| {
            slice
                .try_into()
                .ok()
                .map(|bytes| BalanceAccountNameHash::new(bytes))
        })
        .ok_or(ProgramError::InvalidInstructionData)
}
