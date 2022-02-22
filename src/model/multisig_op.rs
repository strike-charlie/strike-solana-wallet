use crate::error::WalletError;
use crate::instruction::{
    append_instruction, BalanceAccountUpdate, DAppBookUpdate, WalletConfigPolicyUpdate,
    WalletUpdate,
};
use crate::model::address_book::DAppBookEntry;
use crate::model::balance_account::{BalanceAccountGuidHash, BalanceAccountNameHash};
use crate::model::signer::Signer;
use crate::model::wallet::Wallet;
use crate::utils::{pack_option, SlotId};
use arrayref::{array_mut_ref, array_ref, array_refs, mut_array_refs};
use bitvec::macros::internal::funty::Fundamental;
use bytes::BufMut;
use solana_program::account_info::AccountInfo;
use solana_program::clock::Clock;
use solana_program::entrypoint::ProgramResult;
use solana_program::hash::{hash, Hash};
use solana_program::instruction::Instruction;
use solana_program::msg;
use solana_program::program_error::ProgramError;
use solana_program::program_pack::{IsInitialized, Pack, Sealed};
use solana_program::pubkey::{Pubkey, PUBKEY_BYTES};

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub enum ApprovalDisposition {
    NONE = 0,
    APPROVE = 1,
    DENY = 2,
}

impl ApprovalDisposition {
    pub fn from_u8(value: u8) -> ApprovalDisposition {
        match value {
            0 => ApprovalDisposition::NONE,
            1 => ApprovalDisposition::APPROVE,
            2 => ApprovalDisposition::DENY,
            _ => ApprovalDisposition::NONE,
        }
    }

    pub fn to_u8(&self) -> u8 {
        match self {
            ApprovalDisposition::NONE => 0,
            ApprovalDisposition::APPROVE => 1,
            ApprovalDisposition::DENY => 2,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub enum OperationDisposition {
    NONE = 0,
    APPROVED = 1,
    DENIED = 2,
    EXPIRED = 3,
}

impl OperationDisposition {
    pub fn from_u8(value: u8) -> OperationDisposition {
        match value {
            1 => OperationDisposition::APPROVED,
            2 => OperationDisposition::DENIED,
            3 => OperationDisposition::EXPIRED,
            _ => OperationDisposition::NONE,
        }
    }

    pub fn to_u8(&self) -> u8 {
        match self {
            OperationDisposition::NONE => 0,
            OperationDisposition::APPROVED => 1,
            OperationDisposition::DENIED => 2,
            OperationDisposition::EXPIRED => 3,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub struct ApprovalDispositionRecord {
    pub approver: Pubkey,
    pub disposition: ApprovalDisposition,
}

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub enum WrapDirection {
    WRAP = 0,
    UNWRAP = 1,
}

impl WrapDirection {
    pub fn from_u8(value: u8) -> WrapDirection {
        match value {
            0 => WrapDirection::WRAP,
            _ => WrapDirection::UNWRAP,
        }
    }

    pub fn to_u8(&self) -> u8 {
        match self {
            WrapDirection::WRAP => 0,
            WrapDirection::UNWRAP => 1,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub enum SlotUpdateType {
    SetIfEmpty = 0,
    Clear = 1,
}

impl SlotUpdateType {
    pub fn from_u8(value: u8) -> SlotUpdateType {
        match value {
            0 => SlotUpdateType::SetIfEmpty,
            _ => SlotUpdateType::Clear,
        }
    }

    pub fn to_u8(&self) -> u8 {
        match self {
            SlotUpdateType::SetIfEmpty => 0,
            SlotUpdateType::Clear => 1,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
#[repr(u8)]
pub enum BooleanSetting {
    Off = 0,
    On = 1,
}

impl BooleanSetting {
    pub fn from_u8(value: u8) -> BooleanSetting {
        match value {
            0 => BooleanSetting::Off,
            _ => BooleanSetting::On,
        }
    }

    pub fn to_u8(&self) -> u8 {
        match self {
            BooleanSetting::Off => 0,
            BooleanSetting::On => 1,
        }
    }
}

impl Sealed for BooleanSetting {}

impl Default for BooleanSetting {
    fn default() -> Self {
        BooleanSetting::Off
    }
}

impl IsInitialized for BooleanSetting {
    fn is_initialized(&self) -> bool {
        true
    }
}

impl Pack for BooleanSetting {
    const LEN: usize = 1;

    fn pack_into_slice(&self, dst: &mut [u8]) {
        dst[0] = self.to_u8();
    }

    fn unpack_from_slice(src: &[u8]) -> Result<Self, ProgramError> {
        if src.len() == 0 {
            Err(ProgramError::InvalidInstructionData)
        } else {
            Ok(BooleanSetting::from_u8(src[0]))
        }
    }
}

impl ApprovalDispositionRecord {
    pub(crate) const LEN: usize = 1 + PUBKEY_BYTES;

    pub fn pack_into_slice(&self, dst: &mut [u8]) {
        let dst = array_mut_ref![dst, 0, ApprovalDispositionRecord::LEN];
        let (approver_dst, disposition_dst) = mut_array_refs![dst, 32, 1];

        approver_dst.copy_from_slice(&self.approver.to_bytes());
        disposition_dst[0] = self.disposition.to_u8();
    }

    pub fn unpack_from_slice(src: &[u8]) -> Result<Self, ProgramError> {
        let src = array_ref![src, 0, ApprovalDispositionRecord::LEN];
        let (approver_bytes, disposition_bytes) = array_refs![src, 32, 1];

        Ok(ApprovalDispositionRecord {
            approver: Pubkey::new(approver_bytes),
            disposition: ApprovalDisposition::from_u8(disposition_bytes[0]),
        })
    }
}

#[derive(Debug)]
pub struct MultisigOp {
    pub is_initialized: bool,
    pub disposition_records: Vec<ApprovalDispositionRecord>,
    pub dispositions_required: u8,
    pub params_hash: Hash,
    pub started_at: i64,
    pub expires_at: i64,
    pub operation_disposition: OperationDisposition,
}

impl MultisigOp {
    pub fn get_disposition_count(&self, disposition: ApprovalDisposition) -> u8 {
        self.disposition_records
            .iter()
            .filter(|&n| n.disposition == disposition)
            .count() as u8
    }

    pub fn init(
        &mut self,
        approvers: Vec<Pubkey>,
        approvals_required: u8,
        started_at: i64,
        expires_at: i64,
        params: MultisigOpParams,
    ) -> ProgramResult {
        self.disposition_records = approvers
            .iter()
            .map(|approver| ApprovalDispositionRecord {
                approver: *approver,
                disposition: ApprovalDisposition::NONE,
            })
            .collect::<Vec<_>>();
        self.dispositions_required = approvals_required;
        self.params_hash = params.hash();
        self.is_initialized = true;
        self.started_at = started_at;
        self.expires_at = expires_at;
        self.operation_disposition = OperationDisposition::NONE;

        Ok(())
    }

    pub fn validate_and_record_approval_disposition(
        &mut self,
        approver: &AccountInfo,
        disposition: ApprovalDisposition,
        clock: &Clock,
    ) -> ProgramResult {
        if disposition != ApprovalDisposition::APPROVE && disposition != ApprovalDisposition::DENY {
            msg!("Invalid Disposition provided");
            return Err(WalletError::InvalidDisposition.into());
        }

        if !approver.is_signer {
            return Err(WalletError::InvalidSignature.into());
        }

        if let Some(record) = self
            .disposition_records
            .iter_mut()
            .find(|r| r.approver == *approver.key)
        {
            if record.disposition == ApprovalDisposition::NONE {
                record.disposition = disposition
            } else if record.disposition != disposition {
                msg!("Approver already registered a different disposition");
                return Err(WalletError::InvalidDisposition.into());
            }
        } else {
            msg!("Approver is not a configured approver");
            return Err(WalletError::InvalidApprover.into());
        }
        self.update_operation_disposition(clock);

        Ok(())
    }

    pub fn update_operation_disposition(&mut self, clock: &Clock) -> OperationDisposition {
        if self.operation_disposition != OperationDisposition::NONE {
            return self.operation_disposition;
        }
        if clock.unix_timestamp > self.expires_at {
            self.operation_disposition = OperationDisposition::EXPIRED
        } else if self.get_disposition_count(ApprovalDisposition::APPROVE)
            == self.dispositions_required
        {
            self.operation_disposition = OperationDisposition::APPROVED
        } else if self.get_disposition_count(ApprovalDisposition::DENY)
            == self.dispositions_required
        {
            self.operation_disposition = OperationDisposition::DENIED
        }
        return self.operation_disposition;
    }

    pub fn approved(
        &self,
        expected_params: &MultisigOpParams,
        clock: &Clock,
    ) -> Result<bool, ProgramError> {
        if expected_params.hash() != self.params_hash {
            return Err(WalletError::InvalidSignature.into());
        }

        if self.operation_disposition == OperationDisposition::NONE
            && clock.unix_timestamp < self.expires_at
        {
            return Err(WalletError::TransferDispositionNotFinal.into());
        }

        if self.operation_disposition == OperationDisposition::APPROVED {
            return Ok(true);
        }

        Ok(false)
    }
}

impl Sealed for MultisigOp {}

impl IsInitialized for MultisigOp {
    fn is_initialized(&self) -> bool {
        self.is_initialized
    }
}

impl Pack for MultisigOp {
    const LEN: usize =
        1 + ApprovalDispositionRecord::LEN * Wallet::MAX_SIGNERS + 1 + 1 + 32 + 8 + 8 + 1;

    fn pack_into_slice(&self, dst: &mut [u8]) {
        let dst = array_mut_ref![dst, 0, MultisigOp::LEN];
        let (
            is_initialized_dst,
            disposition_records_count_dst,
            disposition_records_dst,
            dispositions_required_dst,
            hash_dst,
            started_at_dst,
            expires_at_dst,
            operation_disposition_dst,
        ) = mut_array_refs![
            dst,
            1,
            1,
            ApprovalDispositionRecord::LEN * Wallet::MAX_SIGNERS,
            1,
            32,
            8,
            8,
            1
        ];

        let MultisigOp {
            is_initialized,
            disposition_records,
            dispositions_required,
            params_hash,
            started_at,
            expires_at,
            operation_disposition,
        } = self;

        is_initialized_dst[0] = *is_initialized as u8;

        disposition_records_count_dst[0] = disposition_records.len() as u8;
        disposition_records_dst.fill(0);
        disposition_records_dst
            .chunks_exact_mut(ApprovalDispositionRecord::LEN)
            .take(disposition_records.len())
            .enumerate()
            .for_each(|(i, chunk)| disposition_records[i].pack_into_slice(chunk));

        dispositions_required_dst[0] = *dispositions_required;

        hash_dst.copy_from_slice(params_hash.to_bytes().as_ref());

        *started_at_dst = started_at.to_le_bytes();
        *expires_at_dst = expires_at.to_le_bytes();

        operation_disposition_dst[0] = operation_disposition.to_u8();
    }

    fn unpack_from_slice(src: &[u8]) -> Result<Self, ProgramError> {
        let src = array_ref![src, 0, MultisigOp::LEN];
        let (
            is_initialized,
            disposition_records_count,
            disposition_record_bytes,
            dispositions_required,
            params_hash,
            started_at,
            expires_at,
            operation_disposition,
        ) = array_refs![
            src,
            1,
            1,
            ApprovalDispositionRecord::LEN * Wallet::MAX_SIGNERS,
            1,
            32,
            8,
            8,
            1
        ];
        let is_initialized = match is_initialized {
            [0] => false,
            [1] => true,
            _ => return Err(ProgramError::InvalidAccountData),
        };

        let disposition_records_count = usize::from(disposition_records_count[0]);
        let mut disposition_records = Vec::with_capacity(Wallet::MAX_SIGNERS);
        disposition_record_bytes
            .chunks_exact(ApprovalDispositionRecord::LEN)
            .take(disposition_records_count)
            .for_each(|chunk| {
                let record = ApprovalDispositionRecord::unpack_from_slice(chunk).unwrap();
                disposition_records.push(record);
            });

        Ok(MultisigOp {
            is_initialized,
            disposition_records,
            dispositions_required: dispositions_required[0],
            params_hash: Hash::new_from_array(*params_hash),
            started_at: i64::from_le_bytes(*started_at),
            expires_at: i64::from_le_bytes(*expires_at),
            operation_disposition: OperationDisposition::from_u8(operation_disposition[0]),
        })
    }
}

// represents multisig operation params that are hashed and signed by the client
#[derive(Debug, PartialEq, Clone)]
pub enum MultisigOpParams {
    UpdateWallet {
        wallet_address: Pubkey,
        update: WalletUpdate,
    },
    CreateBalanceAccount {
        wallet_address: Pubkey,
        account_guid_hash: BalanceAccountGuidHash,
        update: BalanceAccountUpdate,
    },
    UpdateBalanceAccount {
        wallet_address: Pubkey,
        account_guid_hash: BalanceAccountGuidHash,
        update: BalanceAccountUpdate,
    },
    UpdateBalanceAccountNameHash {
        wallet_address: Pubkey,
        account_guid_hash: BalanceAccountGuidHash,
        account_name_hash: BalanceAccountNameHash,
    },
    Transfer {
        wallet_address: Pubkey,
        account_guid_hash: BalanceAccountGuidHash,
        destination: Pubkey,
        amount: u64,
        token_mint: Pubkey,
    },
    Wrap {
        wallet_address: Pubkey,
        account_guid_hash: BalanceAccountGuidHash,
        amount: u64,
        direction: WrapDirection,
    },
    UpdateSigner {
        wallet_address: Pubkey,
        slot_update_type: SlotUpdateType,
        slot_id: SlotId<Signer>,
        signer: Signer,
    },
    UpdateWalletConfigPolicy {
        wallet_address: Pubkey,
        update: WalletConfigPolicyUpdate,
    },
    DAppTransaction {
        wallet_address: Pubkey,
        account_guid_hash: BalanceAccountGuidHash,
        dapp: DAppBookEntry,
        instructions: Vec<Instruction>,
    },
    UpdateAccountSettings {
        wallet_address: Pubkey,
        account_guid_hash: BalanceAccountGuidHash,
        whitelist_enabled: Option<BooleanSetting>,
        dapps_enabled: Option<BooleanSetting>,
    },
    UpdateDAppBook {
        wallet_address: Pubkey,
        update: DAppBookUpdate,
    },
}

impl MultisigOpParams {
    pub fn hash(&self) -> Hash {
        match self {
            MultisigOpParams::UpdateWallet {
                wallet_address,
                update,
            } => {
                let mut update_bytes: Vec<u8> = Vec::new();
                update.pack(&mut update_bytes);

                let mut bytes: Vec<u8> = Vec::new();
                bytes.resize(1 + PUBKEY_BYTES + update_bytes.len(), 0);
                bytes[0] = 0; // type code
                bytes[1..1 + PUBKEY_BYTES].copy_from_slice(&wallet_address.to_bytes());
                bytes[1 + PUBKEY_BYTES..1 + PUBKEY_BYTES + update_bytes.len()]
                    .copy_from_slice(&update_bytes);
                hash(&bytes)
            }
            MultisigOpParams::CreateBalanceAccount {
                wallet_address,
                account_guid_hash,
                update,
            } => {
                let mut update_bytes: Vec<u8> = Vec::new();
                update.pack(&mut update_bytes);

                let mut bytes: Vec<u8> = Vec::new();
                bytes.resize(1 + PUBKEY_BYTES + 32 + update_bytes.len(), 0);
                bytes[0] = 1; // type code
                bytes[1..33].copy_from_slice(&wallet_address.to_bytes());
                bytes[33..65].copy_from_slice(account_guid_hash.to_bytes());
                bytes[65..65 + update_bytes.len()].copy_from_slice(&update_bytes);
                hash(&bytes)
            }
            MultisigOpParams::UpdateBalanceAccount {
                wallet_address,
                account_guid_hash,
                update,
            } => {
                let mut update_bytes: Vec<u8> = Vec::new();
                update.pack(&mut update_bytes);

                let mut bytes: Vec<u8> = Vec::new();
                bytes.resize(1 + PUBKEY_BYTES + 32 + update_bytes.len(), 0);
                bytes[0] = 2; // type code
                bytes[1..33].copy_from_slice(&wallet_address.to_bytes());
                bytes[33..65].copy_from_slice(account_guid_hash.to_bytes());
                bytes[65..65 + update_bytes.len()].copy_from_slice(&update_bytes);
                hash(&bytes)
            }
            MultisigOpParams::Transfer {
                wallet_address,
                account_guid_hash,
                destination,
                amount,
                token_mint,
            } => {
                const LEN: usize = 1 + PUBKEY_BYTES * 4 + 8;
                let mut bytes: [u8; LEN] = [0; LEN];
                let bytes_ref = array_mut_ref![bytes, 0, LEN];
                let (
                    type_code_ref,
                    wallet_address_ref,
                    account_guid_hash_ref,
                    destination_ref,
                    amount_ref,
                    token_mint_ref,
                ) = mut_array_refs![
                    bytes_ref,
                    1,
                    PUBKEY_BYTES,
                    32,
                    PUBKEY_BYTES,
                    8,
                    PUBKEY_BYTES
                ];
                type_code_ref[0] = 3;
                wallet_address_ref.copy_from_slice(wallet_address.as_ref());
                account_guid_hash_ref.copy_from_slice(account_guid_hash.to_bytes());
                destination_ref.copy_from_slice(destination.as_ref());
                *amount_ref = amount.to_le_bytes();
                token_mint_ref.copy_from_slice(token_mint.as_ref());
                hash(&bytes)
            }
            MultisigOpParams::Wrap {
                wallet_address,
                account_guid_hash,
                amount,
                direction,
            } => {
                const LEN: usize = 1 + PUBKEY_BYTES + 32 + 8 + 1;
                let mut bytes: [u8; LEN] = [0; LEN];
                let bytes_ref = array_mut_ref![bytes, 0, LEN];
                let (
                    type_code_ref,
                    wallet_address_ref,
                    account_guid_hash_ref,
                    amount_ref,
                    direction_ref,
                ) = mut_array_refs![bytes_ref, 1, PUBKEY_BYTES, 32, 8, 1];
                type_code_ref[0] = 4;
                wallet_address_ref.copy_from_slice(wallet_address.as_ref());
                account_guid_hash_ref.copy_from_slice(account_guid_hash.to_bytes());
                *amount_ref = amount.to_le_bytes();
                *direction_ref = direction.to_u8().to_le_bytes();
                hash(&bytes)
            }
            MultisigOpParams::UpdateSigner {
                wallet_address,
                slot_update_type,
                slot_id,
                signer,
            } => {
                let mut bytes: Vec<u8> = Vec::new();
                bytes.resize(1 + 2 + PUBKEY_BYTES * 2, 0);
                bytes[0] = 5; // type code
                bytes[1..33].copy_from_slice(&wallet_address.to_bytes());
                bytes[33] = slot_update_type.to_u8();
                bytes[34] = slot_id.value as u8;
                bytes[35..67].copy_from_slice(signer.key.as_ref());
                hash(&bytes)
            }
            MultisigOpParams::DAppTransaction {
                wallet_address,
                account_guid_hash,
                dapp,
                instructions,
            } => {
                let mut bytes: Vec<u8> = Vec::new();
                bytes.push(7);
                bytes.extend_from_slice(&wallet_address.to_bytes());
                bytes.extend_from_slice(&account_guid_hash.to_bytes());
                bytes.put_u16_le(instructions.len().as_u16());
                let mut buf = vec![0; DAppBookEntry::LEN];
                dapp.pack_into_slice(buf.as_mut_slice());
                bytes.extend_from_slice(&buf[..]);
                for instruction in instructions.into_iter() {
                    append_instruction(instruction, &mut bytes);
                }

                hash(&bytes)
            }
            MultisigOpParams::UpdateWalletConfigPolicy {
                wallet_address,
                update,
            } => {
                let mut update_bytes: Vec<u8> = Vec::new();
                update.pack(&mut update_bytes);

                let mut bytes: Vec<u8> = Vec::new();
                bytes.resize(1 + PUBKEY_BYTES + update_bytes.len(), 0);
                bytes[0] = 6; // type code
                bytes[1..1 + PUBKEY_BYTES].copy_from_slice(&wallet_address.to_bytes());
                bytes[1 + PUBKEY_BYTES..1 + PUBKEY_BYTES + update_bytes.len()]
                    .copy_from_slice(&update_bytes);
                hash(&bytes)
            }
            MultisigOpParams::UpdateAccountSettings {
                wallet_address,
                account_guid_hash,
                whitelist_enabled,
                dapps_enabled,
            } => {
                let mut bytes: Vec<u8> = Vec::with_capacity(1 + PUBKEY_BYTES + 32 + 2 + 2);
                bytes.push(8);
                bytes.extend_from_slice(&wallet_address.to_bytes());
                bytes.extend_from_slice(account_guid_hash.to_bytes());
                pack_option(whitelist_enabled.as_ref(), &mut bytes);
                pack_option(dapps_enabled.as_ref(), &mut bytes);
                hash(&bytes)
            }
            MultisigOpParams::UpdateDAppBook {
                wallet_address,
                update,
            } => {
                let mut update_bytes: Vec<u8> = Vec::new();
                update.pack(&mut update_bytes);

                let mut bytes: Vec<u8> = Vec::new();
                bytes.resize(1 + PUBKEY_BYTES + update_bytes.len(), 0);
                bytes[0] = 9; // type code
                bytes[1..1 + PUBKEY_BYTES].copy_from_slice(&wallet_address.to_bytes());
                bytes[1 + PUBKEY_BYTES..1 + PUBKEY_BYTES + update_bytes.len()]
                    .copy_from_slice(&update_bytes);
                hash(&bytes)
            }
            MultisigOpParams::UpdateBalanceAccountNameHash {
                wallet_address,
                account_guid_hash,
                account_name_hash,
            } => {
                let mut bytes: Vec<u8> = Vec::new();
                bytes.resize(97, 0);
                bytes[0] = 10; // type code
                bytes[1..33].copy_from_slice(&wallet_address.to_bytes());
                bytes[33..65].copy_from_slice(account_guid_hash.to_bytes());
                bytes[65..97].copy_from_slice(account_name_hash.to_bytes());
                hash(&bytes)
            }
        }
    }
}
