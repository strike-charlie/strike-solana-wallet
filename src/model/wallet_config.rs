use crate::instruction::WalletConfigUpdate;
use crate::model::program_config::{validate_initiator, ProgramConfig};
use arrayref::{array_mut_ref, array_ref, array_refs, mut_array_refs};
use solana_program::account_info::AccountInfo;
use solana_program::entrypoint::ProgramResult;
use solana_program::msg;
use solana_program::program_error::ProgramError;
use solana_program::program_pack::{IsInitialized, Pack, Sealed};
use solana_program::pubkey::{Pubkey, PUBKEY_BYTES};

#[derive(Debug)]
pub struct WalletConfig {
    pub is_initialized: bool,
    pub program_config_address: Pubkey,
    pub wallet_guid_hash: [u8; 32],
    pub wallet_name_hash: [u8; 32],
    pub approvals_required_for_transfer: u8,
    pub approval_timeout_for_transfer: i64,
    pub approvers: Vec<Pubkey>,
    pub allowed_destinations: Vec<AllowedDestination>,
}

impl WalletConfig {
    pub const MAX_DESTINATIONS: usize = 100;

    pub fn destination_allowed(&self, address: &Pubkey, name_hash: &[u8; 32]) -> bool {
        self.allowed_destinations.contains(&AllowedDestination {
            address: *address,
            name_hash: *name_hash,
        })
    }

    pub fn validate_initiator(
        &self,
        initiator: &AccountInfo,
        assistant_key: &Pubkey,
    ) -> ProgramResult {
        return validate_initiator(initiator, assistant_key, &self.approvers);
    }

    pub fn validate_update(&self, config_update: &WalletConfigUpdate) -> ProgramResult {
        let approvers_after_update = len_after_update(
            &self.approvers,
            &config_update.add_approvers,
            &config_update.remove_approvers,
        );

        let destinations_after_update = len_after_update(
            &self.allowed_destinations,
            &config_update.add_allowed_destinations,
            &config_update.remove_allowed_destinations,
        );

        if approvers_after_update > ProgramConfig::MAX_APPROVERS {
            msg!(
                "Wallet config supports up to {} approvers",
                ProgramConfig::MAX_APPROVERS
            );
            return Err(ProgramError::InvalidArgument);
        }

        if destinations_after_update > WalletConfig::MAX_DESTINATIONS {
            msg!(
                "Wallet config supports up to {} allowed destinations",
                WalletConfig::MAX_DESTINATIONS
            );
            return Err(ProgramError::InvalidArgument);
        }

        if usize::from(config_update.approvals_required_for_transfer) > approvers_after_update {
            msg!(
                "Approvals required for transfer {} can't exceed configured approvers count {}",
                config_update.approvals_required_for_transfer,
                approvers_after_update
            );
            return Err(ProgramError::InvalidArgument);
        }

        Ok(())
    }

    pub fn update(&mut self, config_update: &WalletConfigUpdate) -> ProgramResult {
        self.validate_update(config_update)?;
        self.wallet_name_hash = config_update.name_hash;
        self.approvals_required_for_transfer = config_update.approvals_required_for_transfer;
        if config_update.approval_timeout_for_transfer > 0 {
            self.approval_timeout_for_transfer = config_update.approval_timeout_for_transfer;
        }

        if config_update.add_approvers.len() > 0 || config_update.remove_approvers.len() > 0 {
            for approver_to_remove in &config_update.remove_approvers {
                self.approvers
                    .retain(|approver| approver != approver_to_remove);
            }
            for approver_to_add in &config_update.add_approvers {
                self.approvers.push(*approver_to_add);
            }
        }

        if config_update.add_allowed_destinations.len() > 0
            || config_update.remove_allowed_destinations.len() > 0
        {
            for destination_to_remove in &config_update.remove_allowed_destinations {
                self.allowed_destinations
                    .retain(|destination| destination != destination_to_remove);
            }
            for destination_to_add in &config_update.add_allowed_destinations {
                self.allowed_destinations.push(*destination_to_add);
            }
        }

        Ok(())
    }
}

impl Sealed for WalletConfig {}

impl IsInitialized for WalletConfig {
    fn is_initialized(&self) -> bool {
        self.is_initialized
    }
}

impl Pack for WalletConfig {
    const LEN: usize = 1 + // is_initialized
        PUBKEY_BYTES + // program_config_address
        32 + // guid_hash
        32 + // name_hash
        1 + // approvals_required_for_transfer
        8 + // approval_timeout_for_transfer
        1 + PUBKEY_BYTES * ProgramConfig::MAX_APPROVERS + // approvers with size
        1 + AllowedDestination::LEN * WalletConfig::MAX_DESTINATIONS; // allowed_destinations with size

    fn pack_into_slice(&self, dst: &mut [u8]) {
        let dst = array_mut_ref![dst, 0, WalletConfig::LEN];
        let (
            is_initialized_dst,
            program_config_address_dst,
            guid_hash_dst,
            name_hash_dst,
            approvals_required_for_transfer_dst,
            approval_timeout_for_transfer_dst,
            configured_approvers_count_dst,
            approvers_dst,
            configured_allowed_destinations_count_dst,
            allowed_destinations_dst,
        ) = mut_array_refs![
            dst,
            1,
            PUBKEY_BYTES,
            32,
            32,
            1,
            8,
            1,
            PUBKEY_BYTES * ProgramConfig::MAX_APPROVERS,
            1,
            AllowedDestination::LEN * WalletConfig::MAX_DESTINATIONS
        ];

        is_initialized_dst[0] = self.is_initialized as u8;
        program_config_address_dst.copy_from_slice(&self.program_config_address.to_bytes());

        guid_hash_dst.copy_from_slice(&self.wallet_guid_hash);
        name_hash_dst.copy_from_slice(&self.wallet_name_hash);

        approvals_required_for_transfer_dst[0] = self.approvals_required_for_transfer;
        *approval_timeout_for_transfer_dst = self.approval_timeout_for_transfer.to_le_bytes();

        configured_approvers_count_dst[0] = self.approvers.len() as u8;
        approvers_dst.fill(0);
        approvers_dst
            .chunks_exact_mut(PUBKEY_BYTES)
            .take(self.approvers.len())
            .enumerate()
            .for_each(|(i, chunk)| chunk.copy_from_slice(&self.approvers[i].to_bytes()));

        configured_allowed_destinations_count_dst[0] = self.allowed_destinations.len() as u8;
        allowed_destinations_dst.fill(0);
        allowed_destinations_dst
            .chunks_exact_mut(AllowedDestination::LEN)
            .take(self.allowed_destinations.len())
            .enumerate()
            .for_each(|(i, chunk)| self.allowed_destinations[i].pack_into_slice(chunk));
    }

    fn unpack_from_slice(src: &[u8]) -> Result<Self, ProgramError> {
        let src = array_ref![src, 0, WalletConfig::LEN];
        let (
            is_initialized,
            program_config_address,
            guid_hash,
            name_hash,
            approvals_required_for_transfer,
            approval_timeout_for_transfer,
            configured_approvers_count,
            approvers_bytes,
            configured_allowed_destinations_count,
            allowed_destinations_bytes,
        ) = array_refs![
            src,
            1,
            PUBKEY_BYTES,
            32,
            32,
            1,
            8,
            1,
            PUBKEY_BYTES * ProgramConfig::MAX_APPROVERS,
            1,
            AllowedDestination::LEN * WalletConfig::MAX_DESTINATIONS
        ];
        let is_initialized = match is_initialized {
            [0] => false,
            [1] => true,
            _ => return Err(ProgramError::InvalidAccountData),
        };

        let configured_approvers_count = usize::from(configured_approvers_count[0]);
        let mut approvers = Vec::with_capacity(ProgramConfig::MAX_APPROVERS);
        approvers_bytes
            .chunks_exact(PUBKEY_BYTES)
            .take(configured_approvers_count)
            .for_each(|chunk| {
                let approver = Pubkey::new(chunk);
                approvers.push(approver);
            });

        let configured_allowed_destinations_count =
            usize::from(configured_allowed_destinations_count[0]);
        let mut allowed_destinations = Vec::with_capacity(WalletConfig::MAX_DESTINATIONS);
        allowed_destinations_bytes
            .chunks_exact(AllowedDestination::LEN)
            .take(configured_allowed_destinations_count)
            .for_each(|chunk| {
                let destination = AllowedDestination::unpack_from_slice(chunk).unwrap();
                allowed_destinations.push(destination);
            });

        Ok(WalletConfig {
            is_initialized,
            program_config_address: Pubkey::new(program_config_address),
            wallet_guid_hash: *guid_hash,
            wallet_name_hash: *name_hash,
            approvals_required_for_transfer: approvals_required_for_transfer[0],
            approval_timeout_for_transfer: i64::from_le_bytes(*approval_timeout_for_transfer),
            approvers,
            allowed_destinations,
        })
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub struct AllowedDestination {
    pub address: Pubkey,
    pub name_hash: [u8; 32],
}

impl AllowedDestination {
    pub const LEN: usize = 64;

    pub fn pack_into_slice(&self, dst: &mut [u8]) {
        let dst = array_mut_ref![dst, 0, AllowedDestination::LEN];
        let (address_dst, name_hash_dst) = mut_array_refs![dst, 32, 32];

        address_dst.copy_from_slice(self.address.as_ref());
        name_hash_dst.copy_from_slice(&self.name_hash);
    }

    pub fn unpack_from_slice(src: &[u8]) -> Result<Self, ProgramError> {
        let src = array_ref![src, 0, AllowedDestination::LEN];
        let (address_bytes, name_hash_bytes) = array_refs![src, 32, 32];

        Ok(AllowedDestination {
            address: Pubkey::new_from_array(*address_bytes),
            name_hash: *name_hash_bytes,
        })
    }
}

pub fn len_after_update<A: PartialEq>(
    current_items: &Vec<A>,
    add_items: &Vec<A>,
    remove_items: &Vec<A>,
) -> usize {
    let mut result = 0;
    for item in current_items {
        if !remove_items.contains(&item) {
            result += 1;
        }
    }
    for item_to_add in add_items {
        if !current_items.contains(&item_to_add) {
            result += 1;
        }
    }
    result
}
