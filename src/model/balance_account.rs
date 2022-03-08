use crate::model::address_book::{AddressBook, AddressBookEntry};
use crate::model::multisig_op::BooleanSetting;
use crate::model::wallet::Approvers;
use crate::utils::SlotFlags;
use arrayref::{array_mut_ref, array_ref, array_refs, mut_array_refs};
use solana_program::program_error::ProgramError;
use solana_program::program_pack::{Pack, Sealed};
use std::convert::TryFrom;
use std::time::Duration;

pub type AllowedDestinations = SlotFlags<AddressBookEntry, { AddressBook::FLAGS_STORAGE_SIZE }>;

const WHITELIST_SETTING_BIT: u8 = 0;
const DAPPS_SETTING_BIT: u8 = 1;

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

    pub fn to_bytes(&self) -> &[u8; 32] {
        <&[u8; 32]>::try_from(&self.0[..]).unwrap()
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
    pub whitelist_enabled: BooleanSetting,
    pub dapps_enabled: BooleanSetting,
    pub policy_update_locked: bool,
}

impl Sealed for BalanceAccount {}

impl Pack for BalanceAccount {
    const LEN: usize = 32 + // guid_hash
        32 + // name_hash
        1 + // approvals_required_for_transfer
        8 + // approval_timeout_for_transfer
        Approvers::STORAGE_SIZE + // transfer approvers
        AllowedDestinations::STORAGE_SIZE +  // allowed destinations
        1 + // boolean settings
        1; // policy_update_locked flag

    fn pack_into_slice(&self, dst: &mut [u8]) {
        let dst = array_mut_ref![dst, 0, BalanceAccount::LEN];
        let (
            guid_hash_dst,
            name_hash_dst,
            approvals_required_for_transfer_dst,
            approval_timeout_for_transfer_dst,
            approvers_dst,
            allowed_destinations_dst,
            boolean_settings_dst,
            policy_update_locked_dst,
        ) = mut_array_refs![
            dst,
            32,
            32,
            1,
            8,
            Approvers::STORAGE_SIZE,
            AllowedDestinations::STORAGE_SIZE,
            1,
            1
        ];

        guid_hash_dst.copy_from_slice(&self.guid_hash.0);
        name_hash_dst.copy_from_slice(&self.name_hash.0);

        approvals_required_for_transfer_dst[0] = self.approvals_required_for_transfer;
        *approval_timeout_for_transfer_dst =
            self.approval_timeout_for_transfer.as_secs().to_le_bytes();

        approvers_dst.copy_from_slice(self.transfer_approvers.as_bytes());
        allowed_destinations_dst.copy_from_slice(self.allowed_destinations.as_bytes());
        boolean_settings_dst[0] |= self.whitelist_enabled.to_u8() << WHITELIST_SETTING_BIT;
        boolean_settings_dst[0] |= self.dapps_enabled.to_u8() << DAPPS_SETTING_BIT;
        policy_update_locked_dst[0] = if self.policy_update_locked { 1 } else { 0 }
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
            boolean_settings_src,
            policy_update_locked_src,
        ) = array_refs![
            src,
            32,
            32,
            1,
            8,
            Approvers::STORAGE_SIZE,
            AllowedDestinations::STORAGE_SIZE,
            1,
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
            whitelist_enabled: BooleanSetting::from_u8(
                boolean_settings_src[0] & (1 << WHITELIST_SETTING_BIT),
            ),
            dapps_enabled: BooleanSetting::from_u8(
                boolean_settings_src[0] & (1 << DAPPS_SETTING_BIT),
            ),
            policy_update_locked: if policy_update_locked_src[0] == 1 {
                true
            } else {
                false
            },
        })
    }
}

impl BalanceAccount {
    pub fn is_whitelist_disabled(&self) -> bool {
        return self.whitelist_enabled == BooleanSetting::Off;
    }

    pub fn are_dapps_disabled(&self) -> bool {
        return self.dapps_enabled == BooleanSetting::Off;
    }

    pub fn has_whitelisted_destinations(&self) -> bool {
        return self.allowed_destinations.count_enabled() > 0;
    }
}
