use crate::model::address_book::{AddressBook, AddressBookEntry};
use crate::model::multisig_op::WhitelistStatus;
use crate::model::wallet::Approvers;
use crate::utils::SlotFlags;
use arrayref::{array_mut_ref, array_ref, array_refs, mut_array_refs};
use solana_program::program_error::ProgramError;
use solana_program::program_pack::{Pack, Sealed};
use std::time::Duration;

pub type AllowedDestinations = SlotFlags<AddressBookEntry, { AddressBook::FLAGS_STORAGE_SIZE }>;

#[derive(Debug, Clone, Eq, PartialEq, Copy)]
pub struct BalanceAccountGuidHash([u8; 32]);

impl BalanceAccountGuidHash {
    pub fn new(bytes: &[u8; 32]) -> Self {
        Self(*bytes)
    }

    pub fn zero() -> Self {
        Self::new(&[0; 32])
    }

    pub fn to_bytes(&self) -> &[u8] {
        &self.0[..]
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Copy)]
pub struct BalanceAccountNameHash([u8; 32]);

impl BalanceAccountNameHash {
    pub fn new(bytes: &[u8; 32]) -> Self {
        Self(*bytes)
    }

    pub fn zero() -> Self {
        Self::new(&[0; 32])
    }

    pub fn to_bytes(&self) -> &[u8] {
        &self.0[..]
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct BalanceAccount {
    pub guid_hash: BalanceAccountGuidHash,
    pub name_hash: BalanceAccountNameHash,
    pub approvals_required_for_transfer: u8,
    pub approval_timeout_for_transfer: Duration,
    pub transfer_approvers: Approvers,
    pub allowed_destinations: AllowedDestinations,
    pub whitelist_status: WhitelistStatus,
}

impl Sealed for BalanceAccount {}

impl Pack for BalanceAccount {
    const LEN: usize = 32 + // guid_hash
        32 + // name_hash
        1 + // approvals_required_for_transfer
        8 + // approval_timeout_for_transfer
        Approvers::STORAGE_SIZE + // transfer approvers
        AllowedDestinations::STORAGE_SIZE +  // allowed destinations
        1; // whitelist status

    fn pack_into_slice(&self, dst: &mut [u8]) {
        let dst = array_mut_ref![dst, 0, BalanceAccount::LEN];
        let (
            guid_hash_dst,
            name_hash_dst,
            approvals_required_for_transfer_dst,
            approval_timeout_for_transfer_dst,
            approvers_dst,
            allowed_destinations_dst,
            whitelist_status_dst,
        ) = mut_array_refs![
            dst,
            32,
            32,
            1,
            8,
            Approvers::STORAGE_SIZE,
            AllowedDestinations::STORAGE_SIZE,
            1
        ];

        guid_hash_dst.copy_from_slice(&self.guid_hash.0);
        name_hash_dst.copy_from_slice(&self.name_hash.0);

        approvals_required_for_transfer_dst[0] = self.approvals_required_for_transfer;
        *approval_timeout_for_transfer_dst =
            self.approval_timeout_for_transfer.as_secs().to_le_bytes();

        approvers_dst.copy_from_slice(self.transfer_approvers.as_bytes());
        allowed_destinations_dst.copy_from_slice(self.allowed_destinations.as_bytes());
        whitelist_status_dst[0] = self.whitelist_status.to_u8()
    }

    fn unpack_from_slice(src: &[u8]) -> Result<Self, ProgramError> {
        let src = array_ref![src, 0, BalanceAccount::LEN];
        let (
            guid_hash_src,
            name_hash_src,
            approvals_required_for_transfer_src,
            approval_timeout_for_transfer_src,
            approvers_src,
            allowed_destinations_src,
            whitelist_status_src,
        ) = array_refs![
            src,
            32,
            32,
            1,
            8,
            Approvers::STORAGE_SIZE,
            AllowedDestinations::STORAGE_SIZE,
            1
        ];

        Ok(BalanceAccount {
            guid_hash: BalanceAccountGuidHash(*guid_hash_src),
            name_hash: BalanceAccountNameHash(*name_hash_src),
            approvals_required_for_transfer: approvals_required_for_transfer_src[0],
            approval_timeout_for_transfer: Duration::from_secs(u64::from_le_bytes(
                *approval_timeout_for_transfer_src,
            )),
            transfer_approvers: Approvers::new(*approvers_src),
            allowed_destinations: AllowedDestinations::new(*allowed_destinations_src),
            whitelist_status: WhitelistStatus::from_u8(whitelist_status_src[0]),
        })
    }
}

impl BalanceAccount {
    pub fn is_whitelist_disabled(&self) -> bool {
        return self.whitelist_status == WhitelistStatus::Off;
    }

    pub fn has_whitelisted_destinations(&self) -> bool {
        return self.allowed_destinations.count_enabled() > 0;
    }
}
