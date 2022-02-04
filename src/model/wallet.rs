use crate::error::WalletError;
use crate::instruction::{BalanceAccountUpdate, WalletConfigPolicyUpdate, WalletUpdate};
use crate::model::address_book::{AddressBook, AddressBookEntry, AddressBookEntryNameHash};
use crate::model::balance_account::{
    AllowedDestinations, BalanceAccount, BalanceAccountGuidHash, BalanceAccountNameHash,
};
use crate::model::signer::Signer;
use crate::utils::{GetSlotIds, SlotFlags, SlotId, Slots};
use arrayref::{array_mut_ref, array_ref, array_refs, mut_array_refs};
use itertools::Itertools;
use solana_program::account_info::AccountInfo;
use solana_program::entrypoint::ProgramResult;
use solana_program::msg;
use solana_program::program_error::ProgramError;
use solana_program::program_pack::{IsInitialized, Pack, Sealed};
use solana_program::pubkey::Pubkey;
use std::borrow::BorrowMut;
use std::time::Duration;

pub type Signers = Slots<Signer, { Wallet::MAX_SIGNERS }>;
pub type Approvers = SlotFlags<Signer, { Signers::FLAGS_STORAGE_SIZE }>;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Wallet {
    pub is_initialized: bool,
    pub signers: Signers,
    pub assistant: Signer,
    pub address_book: AddressBook,
    pub approvals_required_for_config: u8,
    pub approval_timeout_for_config: Duration,
    pub config_approvers: Approvers,
    pub balance_accounts: Vec<BalanceAccount>,
    pub config_policy_update_locked: bool,
}

impl Sealed for Wallet {}

impl IsInitialized for Wallet {
    fn is_initialized(&self) -> bool {
        self.is_initialized
    }
}

impl Wallet {
    pub const MAX_BALANCE_ACCOUNTS: usize = 10;
    pub const MAX_SIGNERS: usize = 24;
    pub const MAX_ADDRESS_BOOK_ENTRIES: usize = 128;
    pub const MIN_APPROVAL_TIMEOUT: Duration = Duration::from_secs(60);
    pub const MAX_APPROVAL_TIMEOUT: Duration = Duration::from_secs(60 * 60 * 24 * 365);

    pub fn get_config_approvers_keys(&self) -> Vec<Pubkey> {
        self.get_approvers_keys(&self.config_approvers)
    }

    pub fn get_transfer_approvers_keys(&self, balance_account: &BalanceAccount) -> Vec<Pubkey> {
        self.get_approvers_keys(&balance_account.transfer_approvers)
    }

    fn get_approvers_keys(&self, approvers: &Approvers) -> Vec<Pubkey> {
        approvers
            .iter_enabled()
            .filter_map(|r| self.signers[r].map(|signer| signer.key))
            .collect_vec()
    }

    pub fn get_allowed_destinations(
        &self,
        balance_account: &BalanceAccount,
    ) -> Vec<AddressBookEntry> {
        balance_account
            .allowed_destinations
            .iter_enabled()
            .filter_map(|r| self.address_book[r])
            .collect_vec()
    }

    fn get_balance_account_index(
        &self,
        account_guid_hash: &BalanceAccountGuidHash,
    ) -> Result<usize, ProgramError> {
        self.balance_accounts
            .iter()
            .position(|it| it.guid_hash == *account_guid_hash)
            .ok_or(WalletError::BalanceAccountNotFound.into())
    }

    pub fn get_balance_account(
        &self,
        account_guid_hash: &BalanceAccountGuidHash,
    ) -> Result<&BalanceAccount, ProgramError> {
        Ok(&self.balance_accounts[self.get_balance_account_index(account_guid_hash)?])
    }

    pub fn validate_config_initiator(&self, initiator: &AccountInfo) -> ProgramResult {
        return self.validate_initiator(initiator, || self.get_config_approvers_keys());
    }

    pub fn validate_transfer_initiator(
        &self,
        balance_account: &BalanceAccount,
        initiator: &AccountInfo,
    ) -> ProgramResult {
        return self.validate_initiator(initiator, || {
            self.get_transfer_approvers_keys(balance_account)
        });
    }

    /// Validates the state of a wallet.
    pub fn validate_approval_timeout(timeout: &Duration) -> ProgramResult {
        // approval timeout seconds must fall within program-defined range.
        if *timeout < Wallet::MIN_APPROVAL_TIMEOUT {
            msg!(
                "Approval timeout for config can't be less than {}",
                Wallet::MIN_APPROVAL_TIMEOUT.as_secs(),
            );
            return Err(WalletError::InvalidApprovalTimeout.into());
        }

        if *timeout > Wallet::MAX_APPROVAL_TIMEOUT {
            msg!(
                "Approval timeout for config can't be more than {} seconds",
                Wallet::MAX_APPROVAL_TIMEOUT.as_secs(),
            );
            return Err(WalletError::InvalidApprovalTimeout.into());
        }

        Ok(())
    }

    fn validate_initiator<F: FnOnce() -> Vec<Pubkey>>(
        &self,
        initiator: &AccountInfo,
        get_approvers: F,
    ) -> ProgramResult {
        if !initiator.is_signer {
            return Err(WalletError::InvalidSignature.into());
        }
        if initiator.key == &self.assistant.key || get_approvers().contains(initiator.key) {
            Ok(())
        } else {
            msg!("Transactions can only be initiated by an authorized account");
            Err(ProgramError::InvalidArgument)
        }
    }

    pub fn destination_allowed(
        &self,
        balance_account: &BalanceAccount,
        address: &Pubkey,
        name_hash: &AddressBookEntryNameHash,
    ) -> Result<bool, ProgramError> {
        Ok(
            match self.address_book.find_id(&AddressBookEntry {
                address: *address,
                name_hash: *name_hash,
            }) {
                Some(entry_ref) => balance_account.allowed_destinations.is_enabled(&entry_ref),
                None => false,
            },
        )
    }

    pub fn validate_update(&self, update: &WalletUpdate) -> ProgramResult {
        let mut self_clone = self.clone();
        self_clone.update(update)
    }

    pub fn validate_remove_signer(
        &self,
        signer_to_remove: (SlotId<Signer>, Signer),
    ) -> ProgramResult {
        let mut self_clone = self.clone();
        self_clone.remove_signers(&vec![signer_to_remove])
    }

    pub fn validate_add_signer(&self, signer_to_add: (SlotId<Signer>, Signer)) -> ProgramResult {
        let mut self_clone = self.clone();
        self_clone.add_signers(&vec![signer_to_add])
    }

    pub fn remove_signer(&mut self, signer_to_remove: (SlotId<Signer>, Signer)) -> ProgramResult {
        self.remove_signers(&vec![signer_to_remove])
    }

    pub fn add_signer(&mut self, signer_to_add: (SlotId<Signer>, Signer)) -> ProgramResult {
        self.add_signers(&vec![signer_to_add])
    }

    pub fn update(&mut self, update: &WalletUpdate) -> ProgramResult {
        self.approvals_required_for_config = update.approvals_required_for_config;

        // NOTE: A timeout of 0 means that the existing value should not be updated.
        // Other timeout values are validated below.
        if update.approval_timeout_for_config.as_secs() > 0 {
            self.approval_timeout_for_config = update.approval_timeout_for_config;
        }

        self.disable_config_approvers(&update.remove_config_approvers)?;
        self.remove_signers(&update.remove_signers)?;
        self.add_signers(&update.add_signers)?;
        self.enable_config_approvers(&update.add_config_approvers)?;
        self.remove_address_book_entries(&update.remove_address_book_entries)?;
        self.add_address_book_entries(&update.add_address_book_entries)?;

        let approvers_count_after_update = self.config_approvers.count_enabled();
        if usize::from(update.approvals_required_for_config) > approvers_count_after_update {
            msg!(
                "Approvals required for config {} can't exceed configured approvers count {}",
                update.approvals_required_for_config,
                approvers_count_after_update
            );
            return Err(ProgramError::InvalidArgument);
        }

        Wallet::validate_approval_timeout(&self.approval_timeout_for_config)?;

        if self.approvals_required_for_config == 0 {
            msg!("Approvals required for config can't be 0");
            return Err(ProgramError::InvalidArgument);
        }

        if self.config_approvers.count_enabled() == 0 {
            msg!("At least one config approver has to be configured");
            return Err(ProgramError::InvalidArgument);
        }

        Ok(())
    }

    pub fn validate_config_policy_update(
        &self,
        update: &WalletConfigPolicyUpdate,
    ) -> ProgramResult {
        let mut self_clone = self.clone();
        self_clone.update_config_policy(update)
    }

    pub fn lock_config_policy_updates(&mut self) -> ProgramResult {
        if self.config_policy_update_locked {
            msg!("Only one pending config policy update is allowed at a time");
            return Err(WalletError::ConcurrentOperationsNotAllowed.into());
        }
        self.config_policy_update_locked = true;
        Ok(())
    }

    pub fn unlock_config_policy_updates(&mut self) {
        self.config_policy_update_locked = false;
    }

    pub fn update_config_policy(&mut self, update: &WalletConfigPolicyUpdate) -> ProgramResult {
        self.approvals_required_for_config = update.approvals_required_for_config;
        if update.approval_timeout_for_config.as_secs() > 0 {
            self.approval_timeout_for_config = update.approval_timeout_for_config;
        }

        self.disable_config_approvers(&update.remove_config_approvers)?;
        self.enable_config_approvers(&update.add_config_approvers)?;

        if self.approvals_required_for_config == 0 {
            msg!("Approvals required for config can't be 0");
            return Err(ProgramError::InvalidArgument);
        }

        if self.approval_timeout_for_config.as_secs() == 0 {
            msg!("Approvals timeout for config can't be 0");
            return Err(ProgramError::InvalidArgument);
        }

        let approvers_count = self.config_approvers.count_enabled();
        if usize::from(update.approvals_required_for_config) > approvers_count {
            msg!(
                "Approvals required for config {} can't exceed configured approvers count {}",
                update.approvals_required_for_config,
                approvers_count
            );
            return Err(ProgramError::InvalidArgument);
        }

        Ok(())
    }

    pub fn validate_add_balance_account(
        &self,
        account_guid_hash: &BalanceAccountGuidHash,
        update: &BalanceAccountUpdate,
    ) -> ProgramResult {
        let mut self_clone = self.clone();
        self_clone.add_balance_account(account_guid_hash, update)
    }

    pub fn add_balance_account(
        &mut self,
        account_guid_hash: &BalanceAccountGuidHash,
        update: &BalanceAccountUpdate,
    ) -> ProgramResult {
        let balance_account = BalanceAccount {
            guid_hash: *account_guid_hash,
            name_hash: BalanceAccountNameHash::zero(),
            approvals_required_for_transfer: 0,
            approval_timeout_for_transfer: Duration::from_secs(0),
            transfer_approvers: Approvers::zero(),
            allowed_destinations: AllowedDestinations::zero(),
        };
        self.balance_accounts.push(balance_account);
        self.update_balance_account(account_guid_hash, update)
    }

    pub fn validate_balance_account_update(
        &self,
        account_guid_hash: &BalanceAccountGuidHash,
        update: &BalanceAccountUpdate,
    ) -> ProgramResult {
        let mut self_clone = self.clone();
        self_clone.update_balance_account(account_guid_hash, update)
    }

    pub fn update_balance_account(
        &mut self,
        account_guid_hash: &BalanceAccountGuidHash,
        update: &BalanceAccountUpdate,
    ) -> ProgramResult {
        let balance_account_idx = self.get_balance_account_index(account_guid_hash)?;
        let perform_timeout_update = update.approval_timeout_for_transfer.as_secs() > 0;

        if perform_timeout_update {
            Wallet::validate_approval_timeout(&update.approval_timeout_for_transfer)?;
        }

        self.disable_transfer_approvers(balance_account_idx, &update.remove_transfer_approvers)?;
        self.enable_transfer_approvers(balance_account_idx, &update.add_transfer_approvers)?;
        self.disable_transfer_destinations(
            balance_account_idx,
            &update.remove_allowed_destinations,
        )?;
        self.enable_transfer_destinations(balance_account_idx, &update.add_allowed_destinations)?;

        let balance_account = &mut self.balance_accounts[balance_account_idx].borrow_mut();
        balance_account.name_hash = update.name_hash;
        balance_account.approvals_required_for_transfer = update.approvals_required_for_transfer;

        if perform_timeout_update {
            balance_account.approval_timeout_for_transfer = update.approval_timeout_for_transfer;
        }

        let approvers_count_after_update = balance_account.transfer_approvers.count_enabled();
        if usize::from(update.approvals_required_for_transfer) > approvers_count_after_update {
            msg!(
                "Approvals required for transfer {} can't exceed configured approvers count {}",
                update.approvals_required_for_transfer,
                approvers_count_after_update
            );
            return Err(ProgramError::InvalidArgument);
        }

        if balance_account.approvals_required_for_transfer == 0 {
            msg!("Approvals required for transfer can't be 0");
            return Err(ProgramError::InvalidArgument);
        }

        if balance_account.transfer_approvers.count_enabled() == 0 {
            msg!("At least one transfer approver has to be configured");
            return Err(ProgramError::InvalidArgument);
        }

        Ok(())
    }

    fn add_signers(&mut self, signers_to_add: &Vec<(SlotId<Signer>, Signer)>) -> ProgramResult {
        if !self.signers.can_be_inserted(signers_to_add) {
            msg!("Failed to add signers: at least one of the provided slots is already taken");
            return Err(ProgramError::InvalidArgument);
        }
        self.signers.insert_many(signers_to_add);
        Ok(())
    }

    fn remove_signers(
        &mut self,
        signers_to_remove: &Vec<(SlotId<Signer>, Signer)>,
    ) -> ProgramResult {
        if !self.signers.can_be_removed(signers_to_remove) {
            msg!("Failed to remove signers: at least one of the provided signers is not present in the config");
            return Err(ProgramError::InvalidArgument);
        }
        let slot_ids = signers_to_remove.slot_ids();

        if self.config_approvers.any_enabled(&slot_ids) {
            msg!("Failed to remove signers: not allowed to remove a config approving signer");
            return Err(ProgramError::InvalidArgument);
        };
        for balance_account in &self.balance_accounts {
            if balance_account.transfer_approvers.any_enabled(&slot_ids) {
                msg!("Failed to remove signers: not allowed to remove a transfer approving signer");
                return Err(ProgramError::InvalidArgument);
            }
        }
        self.signers.remove_many(signers_to_remove);
        Ok(())
    }

    fn add_address_book_entries(
        &mut self,
        entries_to_add: &Vec<(SlotId<AddressBookEntry>, AddressBookEntry)>,
    ) -> ProgramResult {
        if !self.address_book.can_be_inserted(entries_to_add) {
            msg!("Failed to add address book entries: at least one of the provided slots is already taken");
            return Err(ProgramError::InvalidArgument);
        }
        self.address_book.insert_many(entries_to_add);
        Ok(())
    }

    fn remove_address_book_entries(
        &mut self,
        entries_to_remove: &Vec<(SlotId<AddressBookEntry>, AddressBookEntry)>,
    ) -> ProgramResult {
        if !self.address_book.can_be_removed(entries_to_remove) {
            msg!("Failed to remove address book entries: at least one of the provided entries is not present in the config");
            return Err(ProgramError::InvalidArgument);
        }
        let slot_ids = entries_to_remove.slot_ids();
        for balance_account in &self.balance_accounts {
            if balance_account.allowed_destinations.any_enabled(&slot_ids) {
                msg!("Failed to remove address book entries: not allowed to remove an allowed address book entry");
                return Err(ProgramError::InvalidArgument);
            }
        }
        self.address_book.remove_many(entries_to_remove);
        Ok(())
    }

    fn enable_config_approvers(
        &mut self,
        approvers: &Vec<(SlotId<Signer>, Signer)>,
    ) -> ProgramResult {
        if !self.signers.contains(approvers) {
            msg!("Failed to enable config approvers: one of the given config approvers is not configured as signer");
            return Err(ProgramError::InvalidArgument);
        }
        self.config_approvers.enable_many(&approvers.slot_ids());
        Ok(())
    }

    fn disable_config_approvers(
        &mut self,
        approvers: &Vec<(SlotId<Signer>, Signer)>,
    ) -> ProgramResult {
        for (id, signer) in approvers {
            if self.signers[*id] == Some(*signer) || self.signers[*id] == None {
                self.config_approvers.disable(id);
            } else {
                msg!("Failed to disable config approvers: unexpected slot value");
                return Err(ProgramError::InvalidArgument);
            }
        }
        Ok(())
    }

    fn enable_transfer_approvers(
        &mut self,
        balance_account_index: usize,
        approvers: &Vec<(SlotId<Signer>, Signer)>,
    ) -> ProgramResult {
        if !self.signers.contains(approvers) {
            msg!("Failed to enable transfer approvers: one of the given transfer approvers is not configured as signer");
            return Err(ProgramError::InvalidArgument);
        }
        self.balance_accounts[balance_account_index]
            .transfer_approvers
            .enable_many(&approvers.slot_ids());
        Ok(())
    }

    fn disable_transfer_approvers(
        &mut self,
        balance_account_index: usize,
        approvers: &Vec<(SlotId<Signer>, Signer)>,
    ) -> ProgramResult {
        for (id, signer) in approvers {
            if self.signers[*id] == Some(*signer) || self.signers[*id] == None {
                self.balance_accounts[balance_account_index]
                    .transfer_approvers
                    .disable(id);
            } else {
                msg!("Failed to disable transfer approvers: unexpected slot value");
                return Err(ProgramError::InvalidArgument);
            }
        }
        Ok(())
    }

    fn enable_transfer_destinations(
        &mut self,
        balance_account_index: usize,
        destinations: &Vec<(SlotId<AddressBookEntry>, AddressBookEntry)>,
    ) -> ProgramResult {
        if !self.address_book.contains(destinations) {
            msg!("Failed to enable transfer destinations: address book does not contain one of the given destinations");
            return Err(ProgramError::InvalidArgument);
        }
        self.balance_accounts[balance_account_index]
            .allowed_destinations
            .enable_many(&destinations.slot_ids());
        Ok(())
    }

    fn disable_transfer_destinations(
        &mut self,
        balance_account_index: usize,
        destinations: &Vec<(SlotId<AddressBookEntry>, AddressBookEntry)>,
    ) -> ProgramResult {
        for (id, address_book_entry) in destinations {
            if self.address_book[*id] == Some(*address_book_entry) || self.address_book[*id] == None
            {
                self.balance_accounts[balance_account_index]
                    .allowed_destinations
                    .disable(id);
            } else {
                msg!("Failed to disable transfer destinations: unexpected slot value");
                return Err(ProgramError::InvalidArgument);
            }
        }
        Ok(())
    }
}

impl Pack for Wallet {
    const LEN: usize = 1 + // is_initialized
        Signers::LEN +
        Signer::LEN + // assistant
        AddressBook::LEN +
        1 + // approvals_required_for_config
        8 + // approval_timeout_for_config
        Approvers::STORAGE_SIZE + // config approvers
        1 + BalanceAccount::LEN * Wallet::MAX_BALANCE_ACCOUNTS + // balance accounts with size
        1; // config_policy_update_locked

    fn pack_into_slice(&self, dst: &mut [u8]) {
        let dst = array_mut_ref![dst, 0, Wallet::LEN];
        let (
            is_initialized_dst,
            signers_dst,
            assistant_account_dst,
            address_book_dst,
            approvals_required_for_config_dst,
            approval_timeout_for_config_dst,
            config_approvers_dst,
            balance_accounts_count_dst,
            balance_accounts_dst,
            config_policy_update_locked_dst,
        ) = mut_array_refs![
            dst,
            1,
            Signers::LEN,
            Signer::LEN,
            AddressBook::LEN,
            1,
            8,
            Approvers::STORAGE_SIZE,
            1,
            BalanceAccount::LEN * Wallet::MAX_BALANCE_ACCOUNTS,
            1
        ];

        is_initialized_dst[0] = self.is_initialized as u8;

        self.signers.pack_into_slice(signers_dst);
        self.assistant.pack_into_slice(assistant_account_dst);
        self.address_book.pack_into_slice(address_book_dst);

        approvals_required_for_config_dst[0] = self.approvals_required_for_config;
        *approval_timeout_for_config_dst = self.approval_timeout_for_config.as_secs().to_le_bytes();

        config_approvers_dst.copy_from_slice(self.config_approvers.as_bytes());

        balance_accounts_count_dst[0] = self.balance_accounts.len() as u8;
        balance_accounts_dst.fill(0);
        balance_accounts_dst
            .chunks_exact_mut(BalanceAccount::LEN)
            .take(self.balance_accounts.len())
            .enumerate()
            .for_each(|(i, chunk)| self.balance_accounts[i].pack_into_slice(chunk));

        config_policy_update_locked_dst[0] = self.config_policy_update_locked as u8;
    }

    fn unpack_from_slice(src: &[u8]) -> Result<Self, ProgramError> {
        let src = array_ref![src, 0, Wallet::LEN];
        let (
            is_initialized,
            signers_src,
            assistant,
            address_book_src,
            approvals_required_for_config,
            approval_timeout_for_config,
            config_approvers_src,
            balance_accounts_count,
            balance_accounts_src,
            config_policy_update_locked_src,
        ) = array_refs![
            src,
            1,
            Signers::LEN,
            Signer::LEN,
            AddressBook::LEN,
            1,
            8,
            Approvers::STORAGE_SIZE,
            1,
            BalanceAccount::LEN * Wallet::MAX_BALANCE_ACCOUNTS,
            1
        ];

        let mut balance_accounts = Vec::with_capacity(Wallet::MAX_BALANCE_ACCOUNTS);
        balance_accounts_src
            .chunks_exact(BalanceAccount::LEN)
            .take(usize::from(balance_accounts_count[0]))
            .for_each(|chunk| {
                balance_accounts.push(BalanceAccount::unpack_from_slice(chunk).unwrap());
            });

        Ok(Wallet {
            is_initialized: match is_initialized {
                [0] => false,
                [1] => true,
                _ => return Err(ProgramError::InvalidAccountData),
            },
            signers: Signers::unpack_from_slice(signers_src)?,
            assistant: Signer::unpack_from_slice(assistant)?,
            address_book: AddressBook::unpack_from_slice(address_book_src)?,
            approvals_required_for_config: approvals_required_for_config[0],
            approval_timeout_for_config: Duration::from_secs(u64::from_le_bytes(
                *approval_timeout_for_config,
            )),
            config_approvers: Approvers::new(*config_approvers_src),
            balance_accounts,
            config_policy_update_locked: match config_policy_update_locked_src {
                [0] => false,
                [1] => true,
                _ => return Err(ProgramError::InvalidAccountData),
            },
        })
    }
}