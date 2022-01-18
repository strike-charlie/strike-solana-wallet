use crate::error::WalletError;
use crate::instruction::ProgramConfigUpdate;
use crate::model::wallet_config::len_after_update;
use arrayref::{array_mut_ref, array_ref, array_refs, mut_array_refs};
use solana_program::account_info::AccountInfo;
use solana_program::entrypoint::ProgramResult;
use solana_program::msg;
use solana_program::program_error::ProgramError;
use solana_program::program_pack::{IsInitialized, Pack, Sealed};
use solana_program::pubkey::{Pubkey, PUBKEY_BYTES};

#[derive(Debug)]
pub struct ProgramConfig {
    pub is_initialized: bool,
    pub approvals_required_for_config: u8,
    pub approval_timeout_for_config: i64,
    pub config_approvers: Vec<Pubkey>,
    pub assistant: Pubkey,
}

impl Sealed for ProgramConfig {}

impl IsInitialized for ProgramConfig {
    fn is_initialized(&self) -> bool {
        self.is_initialized
    }
}

pub fn validate_initiator(
    initiator: &AccountInfo,
    assistant_key: &Pubkey,
    approvers: &Vec<Pubkey>,
) -> ProgramResult {
    if !initiator.is_signer {
        return Err(WalletError::InvalidSignature.into());
    }
    if initiator.key == assistant_key || approvers.contains(initiator.key) {
        Ok(())
    } else {
        msg!("Transactions can only be initiated by an authorized account");
        Err(ProgramError::InvalidArgument)
    }
}

impl ProgramConfig {
    pub const MAX_APPROVERS: usize = 25;

    pub fn validate_initiator(
        &self,
        initiator: &AccountInfo,
        assistant_key: &Pubkey,
    ) -> ProgramResult {
        return validate_initiator(initiator, assistant_key, &self.config_approvers);
    }

    pub fn validate_update(&self, config_update: &ProgramConfigUpdate) -> ProgramResult {
        let approvers_after_update = len_after_update(
            &self.config_approvers,
            &config_update.add_approvers,
            &config_update.remove_approvers,
        );

        if approvers_after_update > ProgramConfig::MAX_APPROVERS {
            msg!(
                "Program config supports up to {} approvers",
                ProgramConfig::MAX_APPROVERS
            );
            return Err(ProgramError::InvalidArgument);
        }

        if usize::from(config_update.approvals_required_for_config) > approvers_after_update {
            msg!(
                "Approvals required for config update {} can't exceed configured approvers count {}",
                config_update.approvals_required_for_config,
                approvers_after_update
            );
            return Err(ProgramError::InvalidArgument);
        }

        Ok(())
    }

    pub fn update(&mut self, config_update: &ProgramConfigUpdate) -> ProgramResult {
        self.validate_update(config_update)?;
        self.approvals_required_for_config = config_update.approvals_required_for_config;
        if config_update.approval_timeout_for_config > 0 {
            self.approval_timeout_for_config = config_update.approval_timeout_for_config;
        }

        if config_update.add_approvers.len() > 0 || config_update.remove_approvers.len() > 0 {
            for approver_to_remove in &config_update.remove_approvers {
                self.config_approvers
                    .retain(|approver| approver != approver_to_remove);
            }
            for approver_to_add in &config_update.add_approvers {
                self.config_approvers.push(*approver_to_add);
            }
        }

        Ok(())
    }
}

impl Pack for ProgramConfig {
    const LEN: usize = 1 + // is_initialized
        1 + // approvals_required_for_config
        8 + // approval_timeout_for_config
        1 + PUBKEY_BYTES * ProgramConfig::MAX_APPROVERS + // config_approvers with size
        PUBKEY_BYTES; // assistant account pubkey

    fn pack_into_slice(&self, dst: &mut [u8]) {
        let dst = array_mut_ref![dst, 0, ProgramConfig::LEN];
        let (
            is_initialized_dst,
            approvals_required_for_config_dst,
            approval_timeout_for_config_dst,
            config_approvers_count_dst,
            config_approvers_dst,
            assistant_account_dst,
        ) = mut_array_refs![
            dst,
            1,
            1,
            8,
            1,
            PUBKEY_BYTES * ProgramConfig::MAX_APPROVERS,
            PUBKEY_BYTES
        ];

        let ProgramConfig {
            is_initialized,
            approvals_required_for_config,
            approval_timeout_for_config,
            config_approvers,
            assistant,
        } = self;

        is_initialized_dst[0] = *is_initialized as u8;
        approvals_required_for_config_dst[0] = *approvals_required_for_config;
        *approval_timeout_for_config_dst = approval_timeout_for_config.to_le_bytes();
        config_approvers_count_dst[0] = config_approvers.len() as u8;
        config_approvers_dst.fill(0);
        config_approvers_dst
            .chunks_exact_mut(PUBKEY_BYTES)
            .take(config_approvers.len())
            .enumerate()
            .for_each(|(i, chunk)| chunk.copy_from_slice(&config_approvers[i].to_bytes()));
        assistant_account_dst.copy_from_slice(&assistant.to_bytes())
    }

    fn unpack_from_slice(src: &[u8]) -> Result<Self, ProgramError> {
        let src = array_ref![src, 0, ProgramConfig::LEN];
        let (
            is_initialized,
            approvals_required_for_config,
            approval_timeout_for_config,
            configured_approvers_count,
            config_approvers_bytes,
            assistant,
        ) = array_refs![
            src,
            1,
            1,
            8,
            1,
            PUBKEY_BYTES * ProgramConfig::MAX_APPROVERS,
            PUBKEY_BYTES
        ];
        let is_initialized = match is_initialized {
            [0] => false,
            [1] => true,
            _ => return Err(ProgramError::InvalidAccountData),
        };

        let config_approvers_count = usize::from(configured_approvers_count[0]);
        let mut config_approvers = Vec::with_capacity(ProgramConfig::MAX_APPROVERS);
        config_approvers_bytes
            .chunks_exact(PUBKEY_BYTES)
            .take(config_approvers_count)
            .for_each(|chunk| {
                let approver = Pubkey::new(chunk);
                config_approvers.push(approver);
            });

        Ok(ProgramConfig {
            is_initialized,
            approvals_required_for_config: approvals_required_for_config[0],
            approval_timeout_for_config: i64::from_le_bytes(*approval_timeout_for_config),
            config_approvers,
            assistant: Pubkey::new_from_array(*assistant),
        })
    }
}
