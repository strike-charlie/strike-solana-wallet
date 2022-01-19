use std::borrow::Borrow;
use std::convert::TryInto;
use std::mem::size_of;
use std::time::Duration;

use solana_program::instruction::Instruction;
use solana_program::program_error::ProgramError;
use solana_program::system_program;
use solana_program::{
    instruction::AccountMeta,
    pubkey::{Pubkey, PUBKEY_BYTES},
    sysvar
};

use crate::model::multisig_op::ApprovalDisposition;
use crate::model::wallet_config::AllowedDestination;

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
    /// 1. `[]` The program config account
    /// 2. `[writable]` The wallet config account
    /// 3. `[signer]` The rent collector account
    FinalizeWalletCreation {
        wallet_guid_hash: [u8; 32],
        config_update: WalletConfigUpdate,
    },

    /// 0  `[writable]` The multisig operation account
    /// 1. `[]` The wallet config account
    /// 2. `[]` The program config account
    /// 3. `[signer]` The initiator account (either the transaction assistant or an approver)
    /// 4. `[]` The sysvar clock account
    InitWalletConfigUpdate {
        wallet_guid_hash: [u8; 32],
        config_update: WalletConfigUpdate,
    },

    /// 0  `[writable]` The multisig operation account
    /// 1. `[writable]` The wallet config account
    /// 2. `[signer]` The rent collector account
    /// 3. `[]` The sysvar clock account
    FinalizeWalletConfigUpdate {
        wallet_guid_hash: [u8; 32],
        config_update: WalletConfigUpdate,
    },

    /// 0  `[writable]` The multisig operation account
    /// 1. `[]` The wallet config account
    /// 2. `[]` The source account
    /// 3. `[]` The destination account
    /// 4. `[signer]` The fee payer account
    /// 5. `[]` The program config account
    /// 6. `[signer]` The initiator account (either the transaction assistant or an approver)
    /// 7. `[]` The sysvar clock account
    InitTransfer {
        amount: u64,
        destination_name_hash: [u8; 32],
        token_mint: Pubkey,
    },

    /// 0  `[writable]` The multisig operation account
    /// 1. `[signer]` The approver account
    /// 2. `[signer]` The fee payer account
    /// 3. `[]` The sysvar clock account
    SetApprovalDisposition { disposition: ApprovalDisposition },

    /// 0  `[writable]` The multisig operation account
    /// 1. `[writable]` The source account
    /// 2. `[writable]` The destination account
    /// 3. `[]` The wallet config account
    /// 4. `[]` The system program
    /// 5. `[signer]` The rent collector account
    /// 6. `[writable]` The source token account, if this is an SPL transfer
    /// 7. `[writable]` The destination token account, if this is an SPL transfer
    /// 8. `[]` The SPL token program account, if this is an SPL transfer
    /// 9. `[]` The token mint authority, if this is an SPL transfer
    /// 10. `[]` The sysvar clock account
    FinalizeTransfer { amount: u64, token_mint: Pubkey },
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
            &ProgramInstruction::SetApprovalDisposition { ref disposition } => {
                buf.push(9);
                buf.push(disposition.to_u8());
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
                ref amount,
                ref destination_name_hash,
                ref token_mint,
            } => {
                buf.push(7);
                buf.extend_from_slice(&amount.to_le_bytes());
                buf.extend_from_slice(destination_name_hash);
                buf.extend_from_slice(&token_mint.to_bytes())
            }
            &ProgramInstruction::FinalizeTransfer {
                ref amount,
                ref token_mint,
            } => {
                buf.push(8);
                buf.extend_from_slice(&amount.to_le_bytes());
                buf.extend_from_slice(&token_mint.to_bytes())
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
        let amount = bytes
            .get(..8)
            .and_then(|slice| slice.try_into().ok())
            .map(u64::from_le_bytes)
            .ok_or(ProgramError::InvalidInstructionData)?;

        let destination_name_hash = bytes
            .get(8..40)
            .and_then(|slice| slice.try_into().ok())
            .ok_or(ProgramError::InvalidInstructionData)?;

        let token_mint = Pubkey::new_from_array(
            bytes
                .get(40..72)
                .and_then(|slice| slice.try_into().ok())
                .ok_or(ProgramError::InvalidInstructionData)?,
        );

        Ok(Self::InitTransfer {
            amount,
            destination_name_hash,
            token_mint,
        })
    }

    fn unpack_set_approval_disposition_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        let (disposition, _) = bytes
            .split_first()
            .ok_or(ProgramError::InvalidInstructionData)?;
        Ok(Self::SetApprovalDisposition {
            disposition: ApprovalDisposition::from_u8(*disposition),
        })
    }

    fn unpack_finalize_transfer_instruction(
        bytes: &[u8],
    ) -> Result<ProgramInstruction, ProgramError> {
        Ok(Self::FinalizeTransfer {
            amount: bytes
                .get(..8)
                .and_then(|slice| slice.try_into().ok())
                .map(u64::from_le_bytes)
                .ok_or(ProgramError::InvalidInstructionData)?,
            token_mint: Pubkey::new_from_array(
                bytes
                    .get(8..40)
                    .and_then(|slice| slice.try_into().ok())
                    .ok_or(ProgramError::InvalidInstructionData)?,
            ),
        })
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ProgramConfigUpdate {
    pub approvals_required_for_config: u8,
    pub approval_timeout_for_config: Duration,
    pub add_approvers: Vec<Pubkey>,
    pub remove_approvers: Vec<Pubkey>,
}

impl ProgramConfigUpdate {
    fn unpack(bytes: &[u8]) -> Result<ProgramConfigUpdate, ProgramError> {
        if bytes.len() < 2 {
            return Err(ProgramError::InvalidInstructionData);
        }
        let (approvals_config_bytes, rest) = bytes.split_at(1);
        let approval_timeout = Duration::from_secs(unpack_timeout(rest)?);
        let (_, rest) = rest.split_at(8);

        let add_approvers = unpack_approvers(rest)?;
        let (_, remove_approvers_bytes) = rest.split_at(add_approvers.len() * PUBKEY_BYTES + 1);
        let remove_approvers = unpack_approvers(remove_approvers_bytes)?;

        Ok(ProgramConfigUpdate {
            approvals_required_for_config: approvals_config_bytes[0],
            approval_timeout_for_config: approval_timeout,
            add_approvers,
            remove_approvers,
        })
    }

    pub fn pack(&self, dst: &mut Vec<u8>) {
        let add_approvers_offset = 9;
        let remove_approvers_offset =
            add_approvers_offset + 1 + self.add_approvers.len() * PUBKEY_BYTES;
        let len = remove_approvers_offset + 1 + self.remove_approvers.len() * PUBKEY_BYTES;

        dst.resize(dst.len() + len, 0);
        dst[0] = self.approvals_required_for_config;
        dst[1..9].copy_from_slice(&self.approval_timeout_for_config.as_secs().to_le_bytes());

        dst[add_approvers_offset] = self.add_approvers.len() as u8;
        dst[add_approvers_offset + 1..remove_approvers_offset]
            .chunks_exact_mut(PUBKEY_BYTES)
            .enumerate()
            .for_each(|(i, chunk)| chunk.copy_from_slice(&self.add_approvers[i].to_bytes()));

        dst[remove_approvers_offset] = self.remove_approvers.len() as u8;
        dst[remove_approvers_offset + 1..len]
            .chunks_exact_mut(PUBKEY_BYTES)
            .enumerate()
            .for_each(|(i, chunk)| chunk.copy_from_slice(&self.remove_approvers[i].to_bytes()));
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct WalletConfigUpdate {
    pub name_hash: [u8; 32],
    pub approvals_required_for_transfer: u8,
    pub approval_timeout_for_transfer: Duration,
    pub add_approvers: Vec<Pubkey>,
    pub remove_approvers: Vec<Pubkey>,
    pub add_allowed_destinations: Vec<AllowedDestination>,
    pub remove_allowed_destinations: Vec<AllowedDestination>,
}

impl WalletConfigUpdate {
    fn unpack(bytes: &[u8]) -> Result<WalletConfigUpdate, ProgramError> {
        if bytes.len() < 1 {
            return Err(ProgramError::InvalidInstructionData);
        }
        let (name_hash_bytes, rest) = bytes.split_at(32);
        let (approvals_config_bytes, rest) = rest.split_at(1);
        let approval_timeout = Duration::from_secs(unpack_timeout(rest)?);
        let (_, rest) = rest.split_at(8);

        let add_approvers = unpack_approvers(rest)?;
        let (_, remove_approvers_bytes) = rest.split_at(add_approvers.len() * PUBKEY_BYTES + 1);
        let remove_approvers = unpack_approvers(remove_approvers_bytes)?;

        let (_, allowed_destinations_bytes) =
            rest.split_at((add_approvers.len() + remove_approvers.len()) * PUBKEY_BYTES + 2);
        let add_allowed_destinations =
            Self::unpack_allowed_destinations(allowed_destinations_bytes)?;
        let (_, remove_allowed_destinations_bytes) = allowed_destinations_bytes
            .split_at(add_allowed_destinations.len() * AllowedDestination::LEN + 1);
        let remove_allowed_destinations =
            Self::unpack_allowed_destinations(remove_allowed_destinations_bytes)?;

        Ok(WalletConfigUpdate {
            name_hash: name_hash_bytes
                .try_into()
                .ok()
                .ok_or(ProgramError::InvalidInstructionData)?,
            approvals_required_for_transfer: approvals_config_bytes[0],
            approval_timeout_for_transfer: approval_timeout,
            add_approvers,
            remove_approvers,
            add_allowed_destinations,
            remove_allowed_destinations,
        })
    }

    fn unpack_allowed_destinations(bytes: &[u8]) -> Result<Vec<AllowedDestination>, ProgramError> {
        let (count, rest) = bytes
            .split_first()
            .ok_or(ProgramError::InvalidInstructionData)?;
        return rest
            .get(0..usize::from(*count) * AllowedDestination::LEN)
            .unwrap()
            .chunks_exact(AllowedDestination::LEN)
            .map(|chunk| AllowedDestination::unpack_from_slice(chunk))
            .collect::<Result<Vec<AllowedDestination>, ProgramError>>();
    }

    pub fn pack(&self, dst: &mut Vec<u8>) {
        let approvals_required_for_transfer_offset = 32;
        let approval_timeout_offset = approvals_required_for_transfer_offset + 1;
        let add_approvers_offset = approval_timeout_offset + 8;
        let remove_approvers_offset =
            add_approvers_offset + 1 + self.add_approvers.len() * PUBKEY_BYTES;
        let add_allowed_destinations_offset =
            remove_approvers_offset + 1 + self.remove_approvers.len() * PUBKEY_BYTES;
        let remove_allowed_destinations_offset = add_allowed_destinations_offset
            + 1
            + self.add_allowed_destinations.len() * AllowedDestination::LEN;
        let len = remove_allowed_destinations_offset
            + 1
            + self.remove_allowed_destinations.len() * AllowedDestination::LEN;

        dst.resize(dst.len() + len, 0);
        dst[0..approvals_required_for_transfer_offset].copy_from_slice(&self.name_hash);
        dst[approvals_required_for_transfer_offset] = self.approvals_required_for_transfer;
        dst[approval_timeout_offset..add_approvers_offset].copy_from_slice(&self.approval_timeout_for_transfer.as_secs().to_le_bytes());

        dst[add_approvers_offset] = self.add_approvers.len() as u8;
        dst[add_approvers_offset + 1..remove_approvers_offset]
            .chunks_exact_mut(PUBKEY_BYTES)
            .enumerate()
            .for_each(|(i, chunk)| chunk.copy_from_slice(&self.add_approvers[i].to_bytes()));

        dst[remove_approvers_offset] = self.remove_approvers.len() as u8;
        dst[remove_approvers_offset + 1..add_allowed_destinations_offset]
            .chunks_exact_mut(PUBKEY_BYTES)
            .enumerate()
            .for_each(|(i, chunk)| chunk.copy_from_slice(&self.remove_approvers[i].to_bytes()));

        dst[add_allowed_destinations_offset] = self.add_allowed_destinations.len() as u8;
        dst[add_allowed_destinations_offset + 1..remove_allowed_destinations_offset]
            .chunks_exact_mut(AllowedDestination::LEN)
            .enumerate()
            .for_each(|(i, chunk)| self.add_allowed_destinations[i].pack_into_slice(chunk));

        dst[remove_allowed_destinations_offset] = self.remove_allowed_destinations.len() as u8;
        dst[remove_allowed_destinations_offset + 1..len]
            .chunks_exact_mut(AllowedDestination::LEN)
            .enumerate()
            .for_each(|(i, chunk)| self.remove_allowed_destinations[i].pack_into_slice(chunk));
    }
}

fn unpack_approvers(bytes: &[u8]) -> Result<Vec<Pubkey>, ProgramError> {
    let (count, rest) = bytes
        .split_first()
        .ok_or(ProgramError::InvalidInstructionData)?;
    let approvers = rest
        .get(0..usize::from(*count) * PUBKEY_BYTES)
        .unwrap()
        .chunks_exact(PUBKEY_BYTES)
        .map(|chunk| Pubkey::new(chunk))
        .collect();
    return Ok(approvers);
}

fn unpack_wallet_guid_hash(bytes: &[u8]) -> Result<[u8; 32], ProgramError> {
    bytes
        .get(..32)
        .and_then(|slice| slice.try_into().ok())
        .ok_or(ProgramError::InvalidInstructionData)
}

fn unpack_timeout(bytes: &[u8]) -> Result<u64, ProgramError> {
    bytes.get(..8)
        .and_then(|slice| slice.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or(ProgramError::InvalidInstructionData)
}

pub fn program_init(
    program_id: &Pubkey,
    program_config_account: &Pubkey,
    assistant_account: &Pubkey,
    config_approvers: Vec<Pubkey>,
    approvals_required_for_config: u8,
    approval_timeout_for_config: Duration,
) -> Instruction {
    let config_update = ProgramConfigUpdate {
        approvals_required_for_config,
        approval_timeout_for_config,
        add_approvers: config_approvers.clone(),
        remove_approvers: Vec::new(),
    };
    let data = ProgramInstruction::Init { config_update }.borrow().pack();

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
    wallet_config_account: Option<&Pubkey>,
) -> Instruction {
    let mut accounts = vec![AccountMeta::new(*multisig_op_account, false)];
    if wallet_config_account.is_some() {
        accounts.push(AccountMeta::new_readonly(
            *wallet_config_account.unwrap(),
            false,
        ))
    }
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
    add_approvers: Vec<Pubkey>,
    remove_approvers: Vec<Pubkey>,
) -> Instruction {
    let config_update = ProgramConfigUpdate {
        approvals_required_for_config,
        approval_timeout_for_config,
        add_approvers: add_approvers.clone(),
        remove_approvers: remove_approvers.clone(),
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
        None,
    )
}

pub fn set_approval_disposition(
    program_id: &Pubkey,
    multisig_op_account: &Pubkey,
    approver: &Pubkey,
    disposition: ApprovalDisposition
) -> Instruction {
    let data = ProgramInstruction::SetApprovalDisposition {
        disposition: disposition,
    }
    .borrow()
    .pack();

    let accounts = vec![
        AccountMeta::new(*multisig_op_account, false),
        AccountMeta::new_readonly(*approver, true),
        AccountMeta::new_readonly(sysvar::clock::id(), false)
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
    approvers: Vec<Pubkey>,
    allowed_destinations: Vec<AllowedDestination>,
) -> Instruction {
    let data = ProgramInstruction::InitWalletCreation {
        wallet_guid_hash,
        config_update: WalletConfigUpdate {
            name_hash,
            approvals_required_for_transfer,
            approval_timeout_for_transfer,
            add_approvers: approvers,
            remove_approvers: vec![],
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
        None,
    )
}

pub fn finalize_wallet_creation(
    program_id: &Pubkey,
    program_config_account: &Pubkey,
    wallet_config_account: &Pubkey,
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
        AccountMeta::new_readonly(*program_config_account, false),
        AccountMeta::new(*wallet_config_account, false),
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
    wallet_account: &Pubkey,
    wallet_guid_hash: [u8; 32],
    name_hash: [u8; 32],
    approvals_required_for_transfer: u8,
    approval_timeout_for_transfer: Duration,
    add_approvers: Vec<Pubkey>,
    remove_approvers: Vec<Pubkey>,
    add_allowed_destinations: Vec<AllowedDestination>,
    remove_allowed_destinations: Vec<AllowedDestination>,
) -> Instruction {
    let data = ProgramInstruction::InitWalletConfigUpdate {
        wallet_guid_hash,
        config_update: WalletConfigUpdate {
            name_hash,
            approvals_required_for_transfer,
            approval_timeout_for_transfer,
            add_approvers,
            remove_approvers,
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
        Some(wallet_account),
    )
}

pub fn finalize_wallet_config_update(
    program_id: &Pubkey,
    wallet_config_account: &Pubkey,
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
        AccountMeta::new(*wallet_config_account, false),
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
    wallet_account: &Pubkey,
    source_account: &Pubkey,
    destination_account: &Pubkey,
    amount: u64,
    destination_name_hash: [u8; 32],
    token_mint: &Pubkey,
) -> Instruction {
    let data = ProgramInstruction::InitTransfer {
        amount,
        destination_name_hash,
        token_mint: *token_mint,
    }
    .borrow()
    .pack();

    let accounts = vec![
        AccountMeta::new(*multisig_op_account, false),
        AccountMeta::new_readonly(*wallet_account, false),
        AccountMeta::new_readonly(*source_account, false),
        AccountMeta::new_readonly(*destination_account, false),
        AccountMeta::new_readonly(*program_config_account, false),
        AccountMeta::new_readonly(*assistant_account, true),
        AccountMeta::new_readonly(sysvar::clock::id(), false)
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
    source_account: &Pubkey,
    destination_account: &Pubkey,
    wallet_config_account: &Pubkey,
    rent_collector_account: &Pubkey,
    amount: u64,
    token_mint: &Pubkey,
    token_authority: Option<&Pubkey>,
) -> Instruction {
    let data = ProgramInstruction::FinalizeTransfer {
        amount,
        token_mint: *token_mint,
    }
    .borrow()
    .pack();

    let mut accounts = vec![
        AccountMeta::new(*multisig_op_account, false),
        AccountMeta::new(*source_account, false),
        AccountMeta::new(*destination_account, false),
        AccountMeta::new_readonly(*wallet_config_account, false),
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
