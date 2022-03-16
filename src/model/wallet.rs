use crate::error::WalletError;
use crate::instruction::{
    AddressBookUpdate, BalanceAccountCreation, BalanceAccountPolicyUpdate, DAppBookUpdate,
    InitialWalletConfig, WalletConfigPolicyUpdate,
};
use crate::model::address_book::{
    AddressBook, AddressBookEntry, AddressBookEntryNameHash, DAppBook, DAppBookEntry,
};
use crate::model::balance_account::{
    AllowedDestinations, BalanceAccount, BalanceAccountGuidHash, BalanceAccountNameHash,
};
use crate::model::multisig_op::BooleanSetting;
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
use std::time::Duration;

pub type Signers = Slots<Signer, { Wallet::MAX_SIGNERS }>;
pub type Approvers = SlotFlags<Signer, { Signers::FLAGS_STORAGE_SIZE }>;
pub type BalanceAccounts = Slots<BalanceAccount, { Wallet::MAX_BALANCE_ACCOUNTS }>;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Wallet {
    pub is_initialized: bool,
    pub signers: Signers,
    pub assistant: Signer,
    pub address_book: AddressBook,
    pub approvals_required_for_config: u8,
    pub approval_timeout_for_config: Duration,
    pub config_approvers: Approvers,
    pub balance_accounts: BalanceAccounts,
    pub config_policy_update_locked: bool,
    pub dapp_book: DAppBook,
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
    pub const MAX_DAPP_BOOK_ENTRIES: usize = 32;

    pub fn get_signers_keys(&self) -> Vec<Pubkey> {
        return self
            .signers
            .filled_slots()
            .iter()
            .map(|signer| signer.1.key)
            .collect_vec();
    }

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

    pub fn get_balance_account(
        &self,
        account_guid_hash: &BalanceAccountGuidHash,
    ) -> Result<BalanceAccount, ProgramError> {
        self.get_balance_account_with_slot_id(account_guid_hash)
            .map(|(_, balance_account)| balance_account)
    }

    pub fn get_balance_account_with_slot_id(
        &self,
        account_guid_hash: &BalanceAccountGuidHash,
    ) -> Result<(SlotId<BalanceAccount>, BalanceAccount), ProgramError> {
        self.balance_accounts
            .find_by(|it| it.guid_hash == *account_guid_hash)
            .ok_or(WalletError::BalanceAccountNotFound.into())
    }

    pub fn validate_config_initiator(&self, initiator: &AccountInfo) -> ProgramResult {
        return self.validate_initiator(initiator, || self.get_signers_keys());
    }

    pub fn validate_transfer_initiator(
        &self,
        _balance_account: &BalanceAccount,
        initiator: &AccountInfo,
    ) -> ProgramResult {
        return self.validate_initiator(initiator, || self.get_signers_keys());
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

    pub fn validate_approvals_required(approvals_required: u8) -> ProgramResult {
        if approvals_required == 0 {
            msg!("Approvals required can't be 0");
            return Err(WalletError::InvalidApproverCount.into());
        }

        Ok(())
    }

    fn validate_initiator<F: FnOnce() -> Vec<Pubkey>>(
        &self,
        initiator: &AccountInfo,
        get_initiators: F,
    ) -> ProgramResult {
        if !initiator.is_signer {
            return Err(WalletError::InvalidSignature.into());
        }
        if initiator.key == &self.assistant.key || get_initiators().contains(initiator.key) {
            Ok(())
        } else {
            msg!("Transactions can only be initiated by an authorized account");
            Err(WalletError::InvalidApprover.into())
        }
    }

    pub fn destination_allowed(
        &self,
        balance_account: &BalanceAccount,
        address: &Pubkey,
        name_hash: &AddressBookEntryNameHash,
    ) -> Result<bool, ProgramError> {
        Ok(balance_account.is_whitelist_disabled()
            || match self.address_book.find_id(&AddressBookEntry {
                address: *address,
                name_hash: *name_hash,
            }) {
                Some(entry_ref) => balance_account.allowed_destinations.is_enabled(&entry_ref),
                None => false,
            })
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

    pub fn initialize(&mut self, initial_config: &InitialWalletConfig) -> ProgramResult {
        self.approvals_required_for_config = initial_config.approvals_required_for_config;

        // NOTE: A timeout of 0 means that the existing value should not be updated.
        // Other timeout values are validated below.
        if initial_config.approval_timeout_for_config.as_secs() > 0 {
            self.approval_timeout_for_config = initial_config.approval_timeout_for_config;
        }

        self.add_signers(&initial_config.signers)?;
        self.enable_config_approvers(&initial_config.config_approvers)?;

        let approvers_count_after_update = self.config_approvers.count_enabled();
        if usize::from(initial_config.approvals_required_for_config) > approvers_count_after_update
        {
            msg!(
                "Approvals required for config {} can't exceed configured approvers count {}",
                initial_config.approvals_required_for_config,
                approvers_count_after_update
            );
            return Err(WalletError::InvalidApproverCount.into());
        }

        Wallet::validate_approval_timeout(&self.approval_timeout_for_config)?;

        if self.approvals_required_for_config == 0 {
            msg!("Approvals required for config can't be 0");
            return Err(WalletError::InvalidApproverCount.into());
        }

        if self.config_approvers.count_enabled() == 0 {
            msg!("At least one config approver has to be configured");
            return Err(WalletError::NoApproversEnabled.into());
        }

        Ok(())
    }

    pub fn validate_address_book_update(&self, update: &AddressBookUpdate) -> ProgramResult {
        let mut self_clone = self.clone();
        self_clone.update_address_book(update)
    }

    pub fn update_address_book(&mut self, update: &AddressBookUpdate) -> ProgramResult {
        self.add_address_book_entries(&update.add_address_book_entries)?;
        for balance_account_whitelist_update in update.balance_account_whitelist_updates.clone() {
            let (slot_id, mut balance_account) =
                self.get_balance_account_with_slot_id(&balance_account_whitelist_update.guid_hash)?;
            self.disable_transfer_destinations(
                &mut balance_account,
                &balance_account_whitelist_update.remove_allowed_destinations,
            )?;
            self.enable_transfer_destinations(
                &mut balance_account,
                &balance_account_whitelist_update.add_allowed_destinations,
            )?;
            self.balance_accounts.replace(slot_id, balance_account);
        }
        self.remove_address_book_entries(&update.remove_address_book_entries)?;
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
        if let Some(approval_timeout_for_config) = update.approval_timeout_for_config {
            Wallet::validate_approval_timeout(&approval_timeout_for_config)?;
            self.approval_timeout_for_config = approval_timeout_for_config;
        }
        if let Some(approvals_required_for_config) = update.approvals_required_for_config {
            self.approvals_required_for_config = approvals_required_for_config;
        }

        self.disable_config_approvers(&update.remove_config_approvers)?;
        self.enable_config_approvers(&update.add_config_approvers)?;

        if self.approvals_required_for_config == 0 {
            msg!("Approvals required for config can't be 0");
            return Err(WalletError::InvalidApproverCount.into());
        }

        let approvers_count = self.config_approvers.count_enabled();
        if usize::from(self.approvals_required_for_config) > approvers_count {
            msg!(
                "Approvals required for config {} can't exceed configured approvers count {}",
                self.approvals_required_for_config,
                approvers_count
            );
            return Err(WalletError::InvalidApproverCount.into());
        }

        Ok(())
    }

    pub fn validate_dapp_book_update(&self, update: &DAppBookUpdate) -> ProgramResult {
        let mut self_clone = self.clone();
        self_clone.update_dapp_book(update)
    }

    pub fn update_dapp_book(&mut self, update: &DAppBookUpdate) -> ProgramResult {
        self.add_dapp_book_entries(&update.add_dapps)?;
        self.remove_dapp_book_entries(&update.remove_dapps)?;

        Ok(())
    }

    pub fn dapp_allowed(&self, dapp: DAppBookEntry) -> bool {
        self.dapp_book.find_id(&dapp).is_some()
    }

    pub fn validate_balance_account_creation(
        &self,
        account_guid_hash: &BalanceAccountGuidHash,
        creation_params: &BalanceAccountCreation,
        program_id: &Pubkey,
    ) -> ProgramResult {
        let mut self_clone = self.clone();
        self_clone.create_balance_account(account_guid_hash, creation_params, program_id)
    }

    pub fn create_balance_account(
        &mut self,
        account_guid_hash: &BalanceAccountGuidHash,
        creation_params: &BalanceAccountCreation,
        program_id: &Pubkey,
    ) -> ProgramResult {
        Wallet::validate_approvals_required(creation_params.approvals_required_for_transfer)?;
        Wallet::validate_approval_timeout(&creation_params.approval_timeout_for_transfer)?;
        if creation_params.approvals_required_for_transfer
            > creation_params.transfer_approvers.len() as u8
        {
            msg!(
                "Approvals required for transfer {} can't exceed configured approvers count {}",
                creation_params.approvals_required_for_transfer,
                creation_params.transfer_approvers.len()
            );
            return Err(WalletError::InvalidApproverCount.into());
        }

        let mut balance_account = BalanceAccount {
            guid_hash: *account_guid_hash,
            name_hash: creation_params.name_hash,
            approvals_required_for_transfer: creation_params.approvals_required_for_transfer,
            approval_timeout_for_transfer: creation_params.approval_timeout_for_transfer,
            transfer_approvers: Approvers::zero(),
            allowed_destinations: AllowedDestinations::zero(),
            whitelist_enabled: creation_params.whitelist_enabled,
            dapps_enabled: creation_params.dapps_enabled,
            policy_update_locked: false,
        };
        self.enable_transfer_approvers(&mut balance_account, &creation_params.transfer_approvers)?;

        self.balance_accounts
            .insert(creation_params.slot_id, balance_account);

        let (source_account_pda, _) =
            Pubkey::find_program_address(&[&account_guid_hash.to_bytes()], program_id);

        self.add_address_book_entries(&vec![(
            creation_params.address_book_slot_id,
            AddressBookEntry {
                address: source_account_pda,
                name_hash: AddressBookEntryNameHash::new(creation_params.name_hash.to_bytes()),
            },
        )])?;
        Ok(())
    }

    pub fn validate_balance_account_policy_update(
        &self,
        account_guid_hash: &BalanceAccountGuidHash,
        update: &BalanceAccountPolicyUpdate,
    ) -> ProgramResult {
        let mut self_clone = self.clone();
        self_clone.update_balance_account_policy(account_guid_hash, update)
    }

    pub fn validate_whitelist_enabled_update(
        &self,
        account_guid_hash: &BalanceAccountGuidHash,
        status: BooleanSetting,
    ) -> ProgramResult {
        let mut self_clone = self.clone();
        self_clone.update_whitelist_enabled(account_guid_hash, status)
    }

    pub fn update_whitelist_enabled(
        &mut self,
        account_guid_hash: &BalanceAccountGuidHash,
        status: BooleanSetting,
    ) -> ProgramResult {
        let (slot_id, mut balance_account) =
            self.get_balance_account_with_slot_id(account_guid_hash)?;

        if status == BooleanSetting::Off {
            if balance_account.has_whitelisted_destinations() {
                msg!("Cannot turn whitelist status to off as there are whitelisted addresses");
                return Err(WalletError::WhitelistedAddressInUse.into());
            }
        }

        balance_account.whitelist_enabled = status;
        self.balance_accounts.replace(slot_id, balance_account);
        Ok(())
    }

    pub fn update_dapps_enabled(
        &mut self,
        account_guid_hash: &BalanceAccountGuidHash,
        enabled: BooleanSetting,
    ) -> ProgramResult {
        let (slot_id, mut balance_account) =
            self.get_balance_account_with_slot_id(account_guid_hash)?;
        balance_account.dapps_enabled = enabled;
        self.balance_accounts.replace(slot_id, balance_account);
        Ok(())
    }

    pub fn update_balance_account_name_hash(
        &mut self,
        account_guid_hash: &BalanceAccountGuidHash,
        account_name_hash: &BalanceAccountNameHash,
    ) -> ProgramResult {
        let (slot_id, mut balance_account) =
            self.get_balance_account_with_slot_id(account_guid_hash)?;
        balance_account.name_hash = account_name_hash.clone();
        self.balance_accounts.replace(slot_id, balance_account);
        Ok(())
    }

    pub fn lock_balance_account_policy_updates(
        &mut self,
        account_guid_hash: &BalanceAccountGuidHash,
    ) -> ProgramResult {
        let (slot_id, mut balance_account) =
            self.get_balance_account_with_slot_id(account_guid_hash)?;

        if balance_account.policy_update_locked {
            msg!("Only one pending policy update is allowed at a time per balance account");
            return Err(WalletError::ConcurrentOperationsNotAllowed.into());
        }
        balance_account.policy_update_locked = true;
        self.balance_accounts.replace(slot_id, balance_account);
        Ok(())
    }

    pub fn unlock_balance_account_policy_updates(
        &mut self,
        account_guid_hash: &BalanceAccountGuidHash,
    ) -> ProgramResult {
        let (slot_id, mut balance_account) =
            self.get_balance_account_with_slot_id(account_guid_hash)?;
        balance_account.policy_update_locked = false;
        self.balance_accounts.replace(slot_id, balance_account);
        Ok(())
    }

    pub fn update_balance_account_policy(
        &mut self,
        account_guid_hash: &BalanceAccountGuidHash,
        update: &BalanceAccountPolicyUpdate,
    ) -> ProgramResult {
        let (slot_id, mut balance_account) =
            self.get_balance_account_with_slot_id(account_guid_hash)?;

        self.disable_transfer_approvers(&mut balance_account, &update.remove_transfer_approvers)?;
        self.enable_transfer_approvers(&mut balance_account, &update.add_transfer_approvers)?;

        if let Some(approval_timeout_for_transfer) = update.approval_timeout_for_transfer {
            Wallet::validate_approval_timeout(&approval_timeout_for_transfer)?;
            balance_account.approval_timeout_for_transfer = approval_timeout_for_transfer;
        }
        if let Some(approvals_required_for_transfer) = update.approvals_required_for_transfer {
            balance_account.approvals_required_for_transfer = approvals_required_for_transfer;
        }

        let approvers_count_after_update = balance_account.transfer_approvers.count_enabled();
        if usize::from(balance_account.approvals_required_for_transfer)
            > approvers_count_after_update
        {
            msg!(
                "Approvals required for transfer {} can't exceed configured approvers count {}",
                balance_account.approvals_required_for_transfer,
                approvers_count_after_update
            );
            return Err(WalletError::InvalidApproverCount.into());
        }

        if balance_account.approvals_required_for_transfer == 0 {
            msg!("Approvals required for transfer can't be 0");
            return Err(WalletError::InvalidApproverCount.into());
        }

        if balance_account.transfer_approvers.count_enabled() == 0 {
            msg!("At least one transfer approver has to be configured");
            return Err(WalletError::NoApproversEnabled.into());
        }

        self.balance_accounts.replace(slot_id, balance_account);
        Ok(())
    }

    fn add_signers(&mut self, signers_to_add: &Vec<(SlotId<Signer>, Signer)>) -> ProgramResult {
        if !self.signers.can_be_inserted(signers_to_add) {
            msg!("Failed to add signers: at least one slot cannot be inserted");
            return Err(WalletError::SlotCannotBeInserted.into());
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
            return Err(WalletError::SlotCannotBeRemoved.into());
        }
        let slot_ids = signers_to_remove.slot_ids();

        if self.config_approvers.any_enabled(&slot_ids) {
            msg!("Failed to remove signers: not allowed to remove a config approving signer");
            return Err(WalletError::SignerIsConfigApprover.into());
        };
        for (_, balance_account) in &self.balance_accounts.filled_slots() {
            if balance_account.transfer_approvers.any_enabled(&slot_ids) {
                msg!("Failed to remove signers: not allowed to remove a transfer approving signer");
                return Err(WalletError::SignerIsTransferApprover.into());
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
            return Err(WalletError::SlotCannotBeInserted.into());
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
            return Err(WalletError::SlotCannotBeRemoved.into());
        }
        let slot_ids = entries_to_remove.slot_ids();
        for (_, balance_account) in &self.balance_accounts.filled_slots() {
            if balance_account.allowed_destinations.any_enabled(&slot_ids) {
                msg!("Failed to remove address book entries: at least one address is currently in use");
                return Err(WalletError::DestinationInUse.into());
            }
        }
        self.address_book.remove_many(entries_to_remove);
        Ok(())
    }

    fn add_dapp_book_entries(
        &mut self,
        entries_to_add: &Vec<(SlotId<DAppBookEntry>, DAppBookEntry)>,
    ) -> ProgramResult {
        if !self.dapp_book.can_be_inserted(entries_to_add) {
            msg!("Failed to add dapp book entries: at least one slot cannot be inserted");
            return Err(WalletError::SlotCannotBeInserted.into());
        }
        self.dapp_book.insert_many(entries_to_add);
        Ok(())
    }

    fn remove_dapp_book_entries(
        &mut self,
        entries_to_remove: &Vec<(SlotId<DAppBookEntry>, DAppBookEntry)>,
    ) -> ProgramResult {
        if !self.dapp_book.can_be_removed(entries_to_remove) {
            msg!("Failed to remove dapp book entries: at least one of the provided entries is not present in the config");
            return Err(WalletError::SlotCannotBeRemoved.into());
        }
        self.dapp_book.remove_many(entries_to_remove);
        Ok(())
    }

    fn enable_config_approvers(
        &mut self,
        approvers: &Vec<(SlotId<Signer>, Signer)>,
    ) -> ProgramResult {
        if !self.signers.contains(approvers) {
            msg!("Failed to enable config approvers: one of the given config approvers is not configured as signer");
            return Err(WalletError::UnknownSigner.into());
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
                return Err(WalletError::InvalidSlot.into());
            }
        }
        Ok(())
    }

    fn enable_transfer_approvers(
        &mut self,
        balance_account: &mut BalanceAccount,
        approvers: &Vec<(SlotId<Signer>, Signer)>,
    ) -> ProgramResult {
        if !self.signers.contains(approvers) {
            msg!("Failed to enable transfer approvers: one of the given transfer approvers is not configured as signer");
            return Err(WalletError::UnknownSigner.into());
        }
        balance_account
            .transfer_approvers
            .enable_many(&approvers.slot_ids());
        Ok(())
    }

    fn disable_transfer_approvers(
        &mut self,
        balance_account: &mut BalanceAccount,
        approvers: &Vec<(SlotId<Signer>, Signer)>,
    ) -> ProgramResult {
        for (id, signer) in approvers {
            if self.signers[*id] == Some(*signer) || self.signers[*id] == None {
                balance_account.transfer_approvers.disable(id);
            } else {
                msg!("Failed to disable transfer approvers: unexpected slot value");
                return Err(WalletError::InvalidSlot.into());
            }
        }
        Ok(())
    }

    fn enable_transfer_destinations(
        &mut self,
        balance_account: &mut BalanceAccount,
        destinations: &Vec<(SlotId<AddressBookEntry>, AddressBookEntry)>,
    ) -> ProgramResult {
        if !self.address_book.contains(destinations) {
            msg!("Failed to enable transfer destinations: address book does not contain one of the given destinations");
            return Err(WalletError::InvalidSlot.into());
        }
        if !destinations.is_empty() && balance_account.is_whitelist_disabled() {
            msg!("Cannot add destinations when whitelisting status is Off");
            return Err(WalletError::WhitelistDisabled.into());
        }
        balance_account
            .allowed_destinations
            .enable_many(&destinations.slot_ids());
        Ok(())
    }

    fn disable_transfer_destinations(
        &mut self,
        balance_account: &mut BalanceAccount,
        destinations: &Vec<(SlotId<AddressBookEntry>, AddressBookEntry)>,
    ) -> ProgramResult {
        for (id, address_book_entry) in destinations {
            if self.address_book[*id] == Some(*address_book_entry) || self.address_book[*id] == None
            {
                balance_account.allowed_destinations.disable(id);
            } else {
                msg!("Failed to disable transfer destinations: unexpected slot value");
                return Err(WalletError::InvalidSlot.into());
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
        1 + // config_policy_update_locked
        DAppBook::LEN +
        BalanceAccounts::LEN;

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
            config_policy_update_locked_dst,
            dapp_book_dst,
            balance_accounts_dst,
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
            DAppBook::LEN,
            BalanceAccounts::LEN
        ];

        is_initialized_dst[0] = self.is_initialized as u8;
        self.signers.pack_into_slice(signers_dst);
        self.assistant.pack_into_slice(assistant_account_dst);
        self.address_book.pack_into_slice(address_book_dst);
        approvals_required_for_config_dst[0] = self.approvals_required_for_config;
        *approval_timeout_for_config_dst = self.approval_timeout_for_config.as_secs().to_le_bytes();
        config_approvers_dst.copy_from_slice(self.config_approvers.as_bytes());
        config_policy_update_locked_dst[0] = self.config_policy_update_locked as u8;
        self.dapp_book.pack_into_slice(dapp_book_dst);
        self.balance_accounts.pack_into_slice(balance_accounts_dst);
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
            config_policy_update_locked_src,
            dapp_book_src,
            balance_accounts_src,
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
            DAppBook::LEN,
            BalanceAccounts::LEN
        ];

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
            balance_accounts: BalanceAccounts::unpack_from_slice(balance_accounts_src)?,
            config_policy_update_locked: match config_policy_update_locked_src {
                [0] => false,
                [1] => true,
                _ => return Err(ProgramError::InvalidAccountData),
            },
            dapp_book: DAppBook::unpack_from_slice(dapp_book_src)?,
        })
    }
}
