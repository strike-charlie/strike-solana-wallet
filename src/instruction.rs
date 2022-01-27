use std::borrow::Borrow;
use std::convert::TryInto;
use std::mem::size_of;
use std::slice::Iter;
use std::time::Duration;

use solana_program::hash::Hash;
use solana_program::program_error::ProgramError;
use solana_program::program_pack::Pack;
use solana_program::{
    instruction::AccountMeta, instruction::Instruction, pubkey::Pubkey, system_program, sysvar,
};

use crate::model::multisig_op::{ApprovalDisposition, WrapDirection};
use crate::model::signer::Signer;
use crate::model::wallet_config::AddressBookEntry;
use crate::utils::SlotId;

#[derive(Debug)]
pub enum ProgramInstruction {
    /// 0. `[writable]` The program config account
    /// 1. `[signer]` The transaction assistant account
    Init { config_update: ProgramConfigUpdate },

    /// 0  `[writable]` The multisig operation account
    /// 1. `[]` The program config account
    /// 2. `[signer]` The initiator account (either the transaction assistant or an approver)
    /// 3. `[]` The sysvar clock account
    InitConfigUpdate { config_update: ProgramConfigUpdate },

    /// 0  `[writable]` The multisig operation account
    /// 1. `[writable]` The program config account
    /// 2. `[signer]` The rent collector account
    FinalizeConfigUpdate { config_update: ProgramConfigUpdate },

    /// 0  `[writable]` The multisig operation account
    /// 1. `[]` The program config account
    /// 2. `[signer]` The initiator account (either the transaction assistant or an approver)
    /// 3. `[]` The sysvar clock account
    InitWalletCreation {
        wallet_guid_hash: [u8; 32],
        config_update: WalletConfigUpdate,
    },

    /// 0  `[writable]` The multisig operation account
    /// 1. `[writable]` The program config account
    /// 2. `[signer]` The rent collector account
    FinalizeWalletCreation {
        wallet_guid_hash: [u8; 32],
        config_update: WalletConfigUpdate,
    },

    /// 0  `[writable]` The multisig operation account
    /// 1. `[]` The program config account
    /// 2. `[signer]` The initiator account (either the transaction assistant or an approver)
    /// 3. `[]` The sysvar clock account
    InitWalletConfigUpdate {
        wallet_guid_hash: [u8; 32],
        config_update: WalletConfigUpdate,
    },

    /// 0  `[writable]` The multisig operation account
    /// 1. `[writable]` The program config account
    /// 2. `[signer]` The rent collector account
    /// 3. `[]` The sysvar clock account
    FinalizeWalletConfigUpdate {
        wallet_guid_hash: [u8; 32],
        config_update: WalletConfigUpdate,
    },

    /// 0  `[writable]` The multisig operation account
    /// 1. `[]` The program config account
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
        wallet_guid_hash: [u8; 32],
        amount: u64,
        destination_name_hash: [u8; 32],
    },

    /// 0  `[writable]` The multisig operation account
    /// 1. `[signer]` The approver account
    /// 2. `[]` The sysvar clock account
    SetApprovalDisposition {
        disposition: ApprovalDisposition,
        params_hash: Hash,
    },

    /// 0  `[writable]` The multisig operation account
    /// 1. `[]` The program config account
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
        wallet_guid_hash: [u8; 32],
        amount: u64,
        token_mint: Pubkey,
    },

    /// 0  `[writable]` The multisig operation account
    /// 1. `[]` The program config account
    /// 2. `[writable]` The account
    /// 3. `[writable]` The associated wrapped SOL account
    /// 4. `[]` The native mint account
    /// 5. `[signer]` The initiator account (either the transaction assistant or an approver)
    /// 6. `[]` The sysvar clock account
    /// 7. `[]` The system program
    /// 8. `[]` The SPL token program
    /// 9. `[]` The Rent sysvar program
    /// 10. `[]` The SPL associated token program
    InitWrapUnwrap {
        wallet_guid_hash: [u8; 32],
        amount: u64,
        direction: WrapDirection,
    },

    /// 0  `[writable]` The multisig operation account
    /// 1. `[]` The program config account
    /// 2. `[writable]` The account
    /// 3. `[]` The system program
    /// 4. `[signer]` The rent collector account
    /// 5. `[]` The sysvar clock account
    /// 6. `[writable]` The wrapped SOL token account
    /// 7. `[]` The SPL token account
    FinalizeWrapUnwrap {
        wallet_guid_hash: [u8; 32],
        amount: u64,
        direction: WrapDirection,
    },
}

impl ProgramInstruction {
    pub fn pack(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(size_of::<Self>());
        match self {
            &ProgramInstruction::Init { ref config_update } => {
                let mut config_update_bytes: Vec<u8> = Vec::new();
                config_update.pack(&mut config_update_bytes);
                buf.push(0);
                buf.extend_from_slice(&config_update_bytes);
            }
            &ProgramInstruction::InitConfigUpdate { ref config_update } => {
                let mut config_update_bytes: Vec<u8> = Vec::new();
                config_update.pack(&mut config_update_bytes);
                buf.push(1);
                buf.extend_from_slice(&config_update_bytes);
            }
            &ProgramInstruction::FinalizeConfigUpdate { ref config_update } => {
                let mut config_update_bytes: Vec<u8> = Vec::new();
                config_update.pack(&mut config_update_bytes);
                buf.push(2);
                buf.extend_from_slice(&config_update_bytes);
            }
            &ProgramInstruction::SetApprovalDisposition {
                ref disposition,
                ref params_hash,
            } => {
                buf.push(9);
                buf.push(disposition.to_u8());
                buf.extend_from_slice(params_hash.as_ref());
            }
            &ProgramInstruction::InitWalletCreation {
                ref wallet_guid_hash,
                ref config_update,
            } => {
                let mut config_update_bytes: Vec<u8> = Vec::new();
                config_update.pack(&mut config_update_bytes);
                buf.push(3);
                buf.extend_from_slice(wallet_guid_hash);
                buf.extend_from_slice(&config_update_bytes);
            }
            &ProgramInstruction::FinalizeWalletCreation {
                ref wallet_guid_hash,
                ref config_update,
            } => {
                let mut config_update_bytes: Vec<u8> = Vec::new();
                config_update.pack(&mut config_update_bytes);
                buf.push(4);
                buf.extend_from_slice(wallet_guid_hash);
                buf.extend_from_slice(&config_update_bytes);
            }
            &ProgramInstruction::InitWalletConfigUpdate {
                ref wallet_guid_hash,
                ref config_update,
            } => {
                let mut config_update_bytes: Vec<u8> = Vec::new();
                config_update.pack(&mut config_update_bytes);
                buf.push(5);
                buf.extend_from_slice(wallet_guid_hash);
                buf.extend_from_slice(&config_update_bytes);
            }
            &ProgramInstruction::FinalizeWalletConfigUpdate {
                ref wallet_guid_hash,
                ref config_update,
            } => {
                let mut config_update_bytes: Vec<u8> = Vec::new();
                config_update.pack(&mut config_update_bytes);
                buf.push(6);
                buf.extend_from_slice(wallet_guid_hash);
                buf.extend_from_slice(&config_update_bytes);
            }
            &ProgramInstruction::InitTransfer {
                ref wallet_guid_hash,
                ref amount,
                ref destination_name_hash,
            } => {
                buf.push(7);
                buf.extend_from_slice(&wallet_guid_hash[..]);
                buf.extend_from_slice(&amount.to_le_bytes());
                buf.extend_from_slice(destination_name_hash);
            }
            &ProgramInstruction::FinalizeTransfer {
                ref wallet_guid_hash,
                ref amount,
                ref token_mint,
            } => {
                buf.push(8);
                buf.extend_from_slice(&wallet_guid_hash[..]);
                buf.extend_from_slice(&amount.to_le_bytes());
                buf.extend_from_slice(&token_mint.to_bytes());
                buf.push(0);
            }
            &ProgramInstruction::InitWrapUnwrap {
                ref wallet_guid_hash,
                ref amount,
                ref direction,
            } => {
                buf.push(10);
                buf.extend_from_slice(&wallet_guid_hash[..]);
                buf.extend_from_slice(&amount.to_le_bytes());
                buf.push(direction.to_u8());
            }
            &ProgramInstruction::FinalizeWrapUnwrap {
                ref wallet_guid_hash,
                ref amount,
                ref direction,
            } => {
                buf.push(11);
                buf.extend_from_slice(&wallet_guid_hash[..]);
                buf.extend_from_slice(&amount.to_le_bytes());
                buf.push(direction.to_u8());
            }
        }
        buf
    }

    pub fn unpack(input: &[u8]) -> Result<Self, ProgramError> {
        let (tag, rest) = input
            .split_first()
            .ok_or(ProgramError::InvalidInstructionData)?;

        Ok(match tag {
            0 => Self::unpack_init_instruction(rest)?,
            1 => Self::unpack_init_config_update_instruction(rest)?,
            2 => Self::unpack_finalize_config_update_instruction(rest)?,
            3 => Self::unpack_init_wallet_creation_instruction(rest)?,
            4 => Self::unpack_finalize_wallet_creation_instruction(rest)?,
            5 => Self::unpack_init_wallet_policy_update_instruction(rest)?,
            6 => Self::unpack_finalize_wallet_policy_update_instruction(rest)?,
            7 => Self::unpack_init_transfer_for_approval_instruction(rest)?,
            8 => Self::unpack_finalize_transfer_instruction(rest)?,
            9 => Self::unpack_set_approval_disposition_instruction(rest)?,
            10 => Self::unpack_init_wrap_unwrap_instruction(rest)?,
            11 => Self::unpack_finalize_wrap_unwrap_instruction(rest)?,
            _ => return Err(ProgramError::InvalidInstructionData),
        })
    }

    fn unpack_init_instruction(bytes: &[u8]) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::Init {
            config_update: ProgramConfigUpdate::unpack(bytes)?,
        })
    }

    fn unpack_init_config_update_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::InitConfigUpdate {
            config_update: ProgramConfigUpdate::unpack(bytes)?,
        })
    }

    fn unpack_finalize_config_update_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::FinalizeConfigUpdate {
            config_update: ProgramConfigUpdate::unpack(bytes)?,
        })
    }

    fn unpack_init_wallet_creation_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::InitWalletCreation {
            wallet_guid_hash: unpack_wallet_guid_hash(bytes)?,
            config_update: WalletConfigUpdate::unpack(
                bytes
                    .get(32..)
                    .ok_or(ProgramError::InvalidInstructionData)?,
            )?,
        })
    }

    fn unpack_finalize_wallet_creation_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::FinalizeWalletCreation {
            wallet_guid_hash: unpack_wallet_guid_hash(bytes)?,
            config_update: WalletConfigUpdate::unpack(
                bytes
                    .get(32..)
                    .ok_or(ProgramError::InvalidInstructionData)?,
            )?,
        })
    }

    fn unpack_init_wallet_policy_update_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::InitWalletConfigUpdate {
            wallet_guid_hash: unpack_wallet_guid_hash(bytes)?,
            config_update: WalletConfigUpdate::unpack(
                bytes
                    .get(32..)
                    .ok_or(ProgramError::InvalidInstructionData)?,
            )?,
        })
    }

    fn unpack_finalize_wallet_policy_update_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::FinalizeWalletConfigUpdate {
            wallet_guid_hash: unpack_wallet_guid_hash(bytes)?,
            config_update: WalletConfigUpdate::unpack(
                bytes
                    .get(32..)
                    .ok_or(ProgramError::InvalidInstructionData)?,
            )?,
        })
    }

    fn unpack_init_transfer_for_approval_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        let wallet_guid_hash = unpack_wallet_guid_hash(bytes)?;

        let amount = bytes
            .get(32..40)
            .and_then(|slice| slice.try_into().ok())
            .map(u64::from_le_bytes)
            .ok_or(ProgramError::InvalidInstructionData)?;

        let destination_name_hash = bytes
            .get(40..72)
            .and_then(|slice| slice.try_into().ok())
            .ok_or(ProgramError::InvalidInstructionData)?;

        Ok(Self::InitTransfer {
            wallet_guid_hash,
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
            wallet_guid_hash: unpack_wallet_guid_hash(bytes)?,
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
        let wallet_guid_hash = unpack_wallet_guid_hash(bytes)?;

        let amount = bytes
            .get(32..40)
            .and_then(|slice| slice.try_into().ok())
            .map(u64::from_le_bytes)
            .ok_or(ProgramError::InvalidInstructionData)?;

        if let Some(direction) = bytes.get(40) {
            Ok(Self::InitWrapUnwrap {
                wallet_guid_hash,
                amount,
                direction: WrapDirection::from_u8(*direction),
            })
        } else {
            Err(ProgramError::InvalidInstructionData)
        }
    }

    fn unpack_finalize_wrap_unwrap_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        let wallet_guid_hash = unpack_wallet_guid_hash(bytes)?;

        let amount = bytes
            .get(32..40)
            .and_then(|slice| slice.try_into().ok())
            .map(u64::from_le_bytes)
            .ok_or(ProgramError::InvalidInstructionData)?;

        if let Some(direction) = bytes.get(40) {
            Ok(Self::FinalizeWrapUnwrap {
                wallet_guid_hash,
                amount,
                direction: WrapDirection::from_u8(*direction),
            })
        } else {
            Err(ProgramError::InvalidInstructionData)
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ProgramConfigUpdate {
    pub approvals_required_for_config: u8,
    pub approval_timeout_for_config: Duration,
    pub add_signers: Vec<(SlotId<Signer>, Signer)>,
    pub remove_signers: Vec<(SlotId<Signer>, Signer)>,
    pub add_config_approvers: Vec<(SlotId<Signer>, Signer)>,
    pub remove_config_approvers: Vec<(SlotId<Signer>, Signer)>,
    pub add_address_book_entries: Vec<(SlotId<AddressBookEntry>, AddressBookEntry)>,
    pub remove_address_book_entries: Vec<(SlotId<AddressBookEntry>, AddressBookEntry)>,
}

impl ProgramConfigUpdate {
    fn unpack(bytes: &[u8]) -> Result<ProgramConfigUpdate, ProgramError> {
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

        Ok(ProgramConfigUpdate {
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
pub struct WalletConfigUpdate {
    pub name_hash: [u8; 32],
    pub approvals_required_for_transfer: u8,
    pub approval_timeout_for_transfer: Duration,
    pub add_transfer_approvers: Vec<(SlotId<Signer>, Signer)>,
    pub remove_transfer_approvers: Vec<(SlotId<Signer>, Signer)>,
    pub add_allowed_destinations: Vec<(SlotId<AddressBookEntry>, AddressBookEntry)>,
    pub remove_allowed_destinations: Vec<(SlotId<AddressBookEntry>, AddressBookEntry)>,
}

impl WalletConfigUpdate {
    fn unpack(bytes: &[u8]) -> Result<WalletConfigUpdate, ProgramError> {
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

        Ok(WalletConfigUpdate {
            name_hash,
            approvals_required_for_transfer,
            approval_timeout_for_transfer,
            add_transfer_approvers: add_approvers,
            remove_transfer_approvers: remove_approvers,
            add_allowed_destinations,
            remove_allowed_destinations,
        })
    }

    pub fn pack(&self, dst: &mut Vec<u8>) {
        dst.extend_from_slice(&self.name_hash);
        dst.push(self.approvals_required_for_transfer);
        append_duration(&self.approval_timeout_for_transfer, dst);
        append_signers(&self.add_transfer_approvers, dst);
        append_signers(&self.remove_transfer_approvers, dst);
        append_address_book_entries(&self.add_allowed_destinations, dst);
        append_address_book_entries(&self.remove_allowed_destinations, dst);
    }
}

fn read_u8<'a>(iter: &'a mut Iter<u8>) -> Option<&'a u8> {
    iter.next()
}

fn read_fixed_size_array<'a, const SIZE: usize>(iter: &'a mut Iter<u8>) -> Option<&'a [u8; SIZE]> {
    read_slice(iter, SIZE).and_then(|slice| slice.try_into().ok())
}

fn read_slice<'a>(iter: &'a mut Iter<u8>, size: usize) -> Option<&'a [u8]> {
    let slice = iter.as_slice().get(0..size);
    if slice.is_some() {
        for _ in 0..size {
            iter.next();
        }
    }
    return slice;
}

fn read_duration(iter: &mut Iter<u8>) -> Option<Duration> {
    read_fixed_size_array::<8>(iter).map(|slice| Duration::from_secs(u64::from_le_bytes(*slice)))
}

fn append_duration(duration: &Duration, dst: &mut Vec<u8>) {
    dst.extend_from_slice(&duration.as_secs().to_le_bytes()[..])
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

fn unpack_wallet_guid_hash(bytes: &[u8]) -> Result<[u8; 32], ProgramError> {
    bytes
        .get(..32)
        .and_then(|slice| slice.try_into().ok())
        .ok_or(ProgramError::InvalidInstructionData)
}

pub fn program_init(
    program_id: &Pubkey,
    program_config_account: &Pubkey,
    assistant_account: &Pubkey,
    signers: Vec<(SlotId<Signer>, Signer)>,
    config_approvers: Vec<(SlotId<Signer>, Signer)>,
    approvals_required_for_config: u8,
    approval_timeout_for_config: Duration,
    address_book: Vec<(SlotId<AddressBookEntry>, AddressBookEntry)>,
) -> Instruction {
    let data = ProgramInstruction::Init {
        config_update: ProgramConfigUpdate {
            approvals_required_for_config,
            approval_timeout_for_config,
            add_signers: signers.clone(),
            remove_signers: Vec::new(),
            add_config_approvers: config_approvers.clone(),
            remove_config_approvers: Vec::new(),
            add_address_book_entries: address_book,
            remove_address_book_entries: Vec::new(),
        },
    }
    .borrow()
    .pack();

    let accounts = vec![
        AccountMeta::new(*program_config_account, false),
        AccountMeta::new_readonly(*assistant_account, true),
    ];

    Instruction {
        program_id: *program_id,
        accounts,
        data,
    }
}

fn init_multisig_op(
    program_id: &Pubkey,
    program_config_account: &Pubkey,
    multisig_op_account: &Pubkey,
    assistant_account: &Pubkey,
    data: Vec<u8>,
) -> Instruction {
    let mut accounts = vec![AccountMeta::new(*multisig_op_account, false)];
    accounts.push(AccountMeta::new_readonly(*program_config_account, false));
    accounts.push(AccountMeta::new_readonly(*assistant_account, true));
    accounts.push(AccountMeta::new_readonly(sysvar::clock::id(), false));

    Instruction {
        program_id: *program_id,
        accounts,
        data,
    }
}

pub fn program_init_config_update(
    program_id: &Pubkey,
    program_config_account: &Pubkey,
    multisig_op_account: &Pubkey,
    assistant_account: &Pubkey,
    approvals_required_for_config: u8,
    approval_timeout_for_config: Duration,
    add_signers: Vec<(SlotId<Signer>, Signer)>,
    remove_signers: Vec<(SlotId<Signer>, Signer)>,
    add_config_approvers: Vec<(SlotId<Signer>, Signer)>,
    remove_config_approvers: Vec<(SlotId<Signer>, Signer)>,
    add_address_book_entries: Vec<(SlotId<AddressBookEntry>, AddressBookEntry)>,
    remove_address_book_entries: Vec<(SlotId<AddressBookEntry>, AddressBookEntry)>,
) -> Instruction {
    let config_update = ProgramConfigUpdate {
        approvals_required_for_config,
        approval_timeout_for_config,
        add_signers: add_signers.clone(),
        remove_signers: remove_signers.clone(),
        add_config_approvers: add_config_approvers.clone(),
        remove_config_approvers: remove_config_approvers.clone(),
        add_address_book_entries: add_address_book_entries.clone(),
        remove_address_book_entries: remove_address_book_entries.clone(),
    };
    let data = ProgramInstruction::InitConfigUpdate { config_update }
        .borrow()
        .pack();
    init_multisig_op(
        program_id,
        program_config_account,
        multisig_op_account,
        assistant_account,
        data,
    )
}

pub fn set_approval_disposition(
    program_id: &Pubkey,
    multisig_op_account: &Pubkey,
    approver: &Pubkey,
    disposition: ApprovalDisposition,
    params_hash: Hash,
) -> Instruction {
    let data = ProgramInstruction::SetApprovalDisposition {
        disposition: disposition,
        params_hash,
    }
    .borrow()
    .pack();

    let accounts = vec![
        AccountMeta::new(*multisig_op_account, false),
        AccountMeta::new_readonly(*approver, true),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
    ];

    Instruction {
        program_id: *program_id,
        accounts,
        data,
    }
}

pub fn finalize_config_update(
    program_id: &Pubkey,
    program_config_account: &Pubkey,
    multisig_op_account: &Pubkey,
    rent_collector_account: &Pubkey,
    config_update: ProgramConfigUpdate,
) -> Instruction {
    let data = ProgramInstruction::FinalizeConfigUpdate { config_update }
        .borrow()
        .pack();
    let accounts = vec![
        AccountMeta::new(*multisig_op_account, false),
        AccountMeta::new(*program_config_account, false),
        AccountMeta::new_readonly(*rent_collector_account, true),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
    ];

    Instruction {
        program_id: *program_id,
        accounts,
        data,
    }
}

pub fn init_wallet_creation(
    program_id: &Pubkey,
    program_config_account: &Pubkey,
    multisig_op_account: &Pubkey,
    assistant_account: &Pubkey,
    wallet_guid_hash: [u8; 32],
    name_hash: [u8; 32],
    approvals_required_for_transfer: u8,
    approval_timeout_for_transfer: Duration,
    approvers: Vec<(SlotId<Signer>, Signer)>,
    allowed_destinations: Vec<(SlotId<AddressBookEntry>, AddressBookEntry)>,
) -> Instruction {
    let data = ProgramInstruction::InitWalletCreation {
        wallet_guid_hash,
        config_update: WalletConfigUpdate {
            name_hash,
            approvals_required_for_transfer,
            approval_timeout_for_transfer,
            add_transfer_approvers: approvers.clone(),
            remove_transfer_approvers: vec![],
            add_allowed_destinations: allowed_destinations,
            remove_allowed_destinations: vec![],
        },
    }
    .borrow()
    .pack();
    init_multisig_op(
        program_id,
        program_config_account,
        multisig_op_account,
        assistant_account,
        data,
    )
}

pub fn finalize_wallet_creation(
    program_id: &Pubkey,
    program_config_account: &Pubkey,
    multisig_op_account: &Pubkey,
    rent_collector_account: &Pubkey,
    wallet_guid_hash: [u8; 32],
    config_update: WalletConfigUpdate,
) -> Instruction {
    let data = ProgramInstruction::FinalizeWalletCreation {
        wallet_guid_hash,
        config_update,
    }
    .borrow()
    .pack();
    let accounts = vec![
        AccountMeta::new(*multisig_op_account, false),
        AccountMeta::new(*program_config_account, false),
        AccountMeta::new_readonly(*rent_collector_account, true),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
    ];

    Instruction {
        program_id: *program_id,
        accounts,
        data,
    }
}

pub fn init_wallet_config_update(
    program_id: &Pubkey,
    program_config_account: &Pubkey,
    multisig_op_account: &Pubkey,
    assistant_account: &Pubkey,
    wallet_guid_hash: [u8; 32],
    name_hash: [u8; 32],
    approvals_required_for_transfer: u8,
    approval_timeout_for_transfer: Duration,
    add_transfer_approvers: Vec<(SlotId<Signer>, Signer)>,
    remove_transfer_approvers: Vec<(SlotId<Signer>, Signer)>,
    add_allowed_destinations: Vec<(SlotId<AddressBookEntry>, AddressBookEntry)>,
    remove_allowed_destinations: Vec<(SlotId<AddressBookEntry>, AddressBookEntry)>,
) -> Instruction {
    let data = ProgramInstruction::InitWalletConfigUpdate {
        wallet_guid_hash,
        config_update: WalletConfigUpdate {
            name_hash,
            approvals_required_for_transfer,
            approval_timeout_for_transfer,
            add_transfer_approvers,
            remove_transfer_approvers,
            add_allowed_destinations,
            remove_allowed_destinations,
        },
    }
    .borrow()
    .pack();
    init_multisig_op(
        program_id,
        program_config_account,
        multisig_op_account,
        assistant_account,
        data,
    )
}

pub fn finalize_wallet_config_update(
    program_id: &Pubkey,
    program_config_account: &Pubkey,
    multisig_op_account: &Pubkey,
    rent_collector_account: &Pubkey,
    wallet_guid_hash: [u8; 32],
    config_update: WalletConfigUpdate,
) -> Instruction {
    let data = ProgramInstruction::FinalizeWalletConfigUpdate {
        wallet_guid_hash,
        config_update,
    }
    .borrow()
    .pack();
    let accounts = vec![
        AccountMeta::new(*multisig_op_account, false),
        AccountMeta::new(*program_config_account, false),
        AccountMeta::new_readonly(*rent_collector_account, true),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
    ];

    Instruction {
        program_id: *program_id,
        accounts,
        data,
    }
}

pub fn init_transfer(
    program_id: &Pubkey,
    program_config_account: &Pubkey,
    multisig_op_account: &Pubkey,
    assistant_account: &Pubkey,
    source_account: &Pubkey,
    destination_account: &Pubkey,
    wallet_guid_hash: [u8; 32],
    amount: u64,
    destination_name_hash: [u8; 32],
    token_mint: &Pubkey,
    fee_payer: &Pubkey,
) -> Instruction {
    let data = ProgramInstruction::InitTransfer {
        wallet_guid_hash,
        amount,
        destination_name_hash,
    }
    .borrow()
    .pack();

    let destination_token_account =
        spl_associated_token_account::get_associated_token_address(destination_account, token_mint);

    let accounts = vec![
        AccountMeta::new(*multisig_op_account, false),
        AccountMeta::new_readonly(*program_config_account, false),
        AccountMeta::new(*source_account, false),
        AccountMeta::new_readonly(*destination_account, false),
        AccountMeta::new_readonly(*assistant_account, true),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
        AccountMeta::new_readonly(*token_mint, false),
        AccountMeta::new(destination_token_account, false),
        AccountMeta::new(*fee_payer, true),
        AccountMeta::new_readonly(system_program::id(), false),
        AccountMeta::new_readonly(spl_token::id(), false),
        AccountMeta::new_readonly(sysvar::rent::id(), false),
        AccountMeta::new_readonly(spl_associated_token_account::id(), false),
    ];

    Instruction {
        program_id: *program_id,
        accounts,
        data,
    }
}

pub fn finalize_transfer(
    program_id: &Pubkey,
    multisig_op_account: &Pubkey,
    program_config_account: &Pubkey,
    source_account: &Pubkey,
    destination_account: &Pubkey,
    rent_collector_account: &Pubkey,
    wallet_guid_hash: [u8; 32],
    amount: u64,
    token_mint: &Pubkey,
    token_authority: Option<&Pubkey>,
) -> Instruction {
    let data = ProgramInstruction::FinalizeTransfer {
        wallet_guid_hash,
        amount,
        token_mint: *token_mint,
    }
    .borrow()
    .pack();
    let mut accounts = vec![
        AccountMeta::new(*multisig_op_account, false),
        AccountMeta::new_readonly(*program_config_account, false),
        AccountMeta::new(*source_account, false),
        AccountMeta::new(*destination_account, false),
        AccountMeta::new_readonly(system_program::id(), false),
        AccountMeta::new_readonly(*rent_collector_account, true),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
    ];
    if *token_mint != system_program::id() {
        // SPL
        accounts.extend_from_slice(&[
            AccountMeta::new(
                spl_associated_token_account::get_associated_token_address(
                    source_account,
                    &token_mint,
                ),
                false,
            ),
            AccountMeta::new(
                spl_associated_token_account::get_associated_token_address(
                    destination_account,
                    &token_mint,
                ),
                false,
            ),
            AccountMeta::new_readonly(spl_token::id(), false),
            AccountMeta::new_readonly(*token_authority.unwrap(), false),
        ])
    }
    Instruction {
        program_id: *program_id,
        accounts,
        data,
    }
}

pub fn init_wrap_unwrap(
    program_id: &Pubkey,
    program_config_account: &Pubkey,
    multisig_op_account: &Pubkey,
    assistant_account: &Pubkey,
    account: &Pubkey,
    wallet_guid_hash: [u8; 32],
    amount: u64,
    direction: WrapDirection,
) -> Instruction {
    let data = ProgramInstruction::InitWrapUnwrap {
        wallet_guid_hash,
        amount,
        direction,
    }
    .borrow()
    .pack();

    let wrapped_sol_account = spl_associated_token_account::get_associated_token_address(
        account,
        &spl_token::native_mint::id(),
    );

    let accounts = vec![
        AccountMeta::new(*multisig_op_account, false),
        AccountMeta::new_readonly(*program_config_account, false),
        AccountMeta::new(*account, false),
        AccountMeta::new(wrapped_sol_account, false),
        AccountMeta::new_readonly(spl_token::native_mint::id(), false),
        AccountMeta::new_readonly(*assistant_account, true),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
        AccountMeta::new_readonly(system_program::id(), false),
        AccountMeta::new_readonly(spl_token::id(), false),
        AccountMeta::new_readonly(sysvar::rent::id(), false),
        AccountMeta::new_readonly(spl_associated_token_account::id(), false),
    ];

    Instruction {
        program_id: *program_id,
        accounts,
        data,
    }
}

pub fn finalize_wrap_unwrap(
    program_id: &Pubkey,
    multisig_op_account: &Pubkey,
    program_config_account: &Pubkey,
    account: &Pubkey,
    rent_collector_account: &Pubkey,
    wallet_guid_hash: [u8; 32],
    amount: u64,
    direction: WrapDirection,
) -> Instruction {
    let data = ProgramInstruction::FinalizeWrapUnwrap {
        wallet_guid_hash,
        amount,
        direction,
    }
    .borrow()
    .pack();

    let wrapped_sol_account = spl_associated_token_account::get_associated_token_address(
        account,
        &spl_token::native_mint::id(),
    );

    let accounts = vec![
        AccountMeta::new(*multisig_op_account, false),
        AccountMeta::new_readonly(*program_config_account, false),
        AccountMeta::new(*account, false),
        AccountMeta::new_readonly(system_program::id(), false),
        AccountMeta::new_readonly(*rent_collector_account, true),
        AccountMeta::new_readonly(sysvar::clock::id(), false),
        AccountMeta::new(wrapped_sol_account, false),
        AccountMeta::new_readonly(spl_token::id(), false),
    ];

    Instruction {
        program_id: *program_id,
        accounts,
        data,
    }
}
