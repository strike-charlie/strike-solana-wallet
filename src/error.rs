use solana_program::program_error::ProgramError;
use thiserror::Error;

#[derive(Error, Debug, Copy, Clone)]
pub enum WalletError {
    // 0
    /// Unexpected account in instruction.
    #[error("Account Not Recognized")]
    AccountNotRecognized,
    /// Source account for transfer not recognized.
    #[error("Invalid Source Account")]
    InvalidSourceAccount,
    /// Approver account is not configured as a Signer or, more generally, a
    /// required signature is invalid or missing.
    #[error("Invalid Signature")]
    InvalidSignature,
    /// Account included in instruction is not a valid Approver.
    #[error("Invalid Approver")]
    InvalidApprover,
    /// The disposition of an Approval is invalid for the executing instruction.
    #[error("Invalid Approval Disposition")]
    InvalidDisposition,

    // 5
    /// Attempting to set Approval timeout beyond allowed min and max.
    #[error("Invalid Approval Timeout")]
    InvalidApprovalTimeout,
    /// Tried to set the number of approvals required to invalid number, like
    /// zero or more than the total number of Signers available.
    #[error("Invalid Approver Count")]
    InvalidApproverCount,
    /// Tried to access an element of a collection that is either out-of-bounds
    /// or unauthorized. E.G.: AddressBook entries, Transfer Approvers.
    #[error("Invalid Slot")]
    InvalidSlot,
    /// TransferDispositionNotFinal used internally in MultisigOp finalization.
    #[error("Transfer Disposition Not Final")]
    TransferDispositionNotFinal,
    /// Attempting to transfer an invalid amount between accounts.
    #[error("Amount Overflow")]
    AmountOverflow,

    // 10
    /// Insufficient balance for a transfer.
    #[error("InSufficient Balance")]
    InsufficientBalance,
    /// Destination address not allowed in a transfer.
    #[error("Destination Not Allowed")]
    DestinationNotAllowed,
    /// Balance Account referenced by an instruction does not exist.
    #[error("Balance Account Not Found")]
    BalanceAccountNotFound,
    /// Invalid SPL source token account.
    #[error("Invalid Source Token Account")]
    InvalidSourceTokenAccount,
    /// Invalid SPL destination token account.
    #[error("Invalid Destination Token Account")]
    InvalidDestinationTokenAccount,

    // 15
    /// Invalid SPL token mint account.
    #[error("Invalid Token Mint Account")]
    InvalidTokenMintAccount,
    /// Only one policy config change can be initiated at a time.
    #[error("Concurrent Operations Not Allowed")]
    ConcurrentOperationsNotAllowed,
    /// Simulation of MultisigOp finalization completed normally.
    #[error("Simulation Finished Successfully")]
    SimulationFinished,
    /// Cannot whitelist an address when Whitelisting is not enabled.
    #[error("Whitelist Is Disabled")]
    WhitelistDisabled,
    /// Cannot disable Whitelisting while one or more address is whitelisted.
    #[error("Whitelisting In Use")]
    WhitelistedAddressInUse,

    // 20
    /// The set of Approvers for transfers or config changes must not be empty.
    #[error("No Approvers Enabled")]
    NoApproversEnabled,
    /// DApp transactions are disabled.
    #[error("DApp Transactions Are Disabled")]
    DAppsDisabled,
    /// Destination Already In Use
    #[error("Destination Already In Use")]
    DestinationInUse,
    /// Unknown Signer
    #[error("Unknown Signer")]
    UnknownSigner,
    /// DApp Not Allowed
    #[error("DApp Not Allowed")]
    DAppNotAllowed,

    // 25
    /// Slot Cannot Be Inserted
    #[error("Slot Cannot Be Inserted")]
    SlotCannotBeInserted,
    /// Slot Cannot Be Removed
    #[error("Slot Cannot Be Removed")]
    SlotCannotBeRemoved,
    /// Signer Is A Config Approver
    #[error("Signer Is A Config Approver")]
    SignerIsConfigApprover,
    /// Signer Is A Transfer Approver
    #[error("Signer Is A Transfer Approver")]
    SignerIsTransferApprover,
    /// Invalid PDA address or bump seed
    #[error("Invalid PDA")]
    InvalidPDA,
}

impl From<WalletError> for ProgramError {
    fn from(e: WalletError) -> Self {
        ProgramError::Custom(e as u32)
    }
}
