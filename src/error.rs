use solana_program::program_error::ProgramError;
use thiserror::Error;

#[derive(Error, Debug, Copy, Clone)]
pub enum WalletError {
    #[error("Invalid Source Account")]
    InvalidSourceAccount,
    #[error("Invalid Signature")]
    InvalidSignature,
    #[error("Invalid Approver")]
    InvalidApprover,
    #[error("Invalid Disposition")]
    InvalidDisposition,
    #[error("Transfer Disposition Not Final")]
    TransferDispositionNotFinal,
    #[error("Amount Overflow")]
    AmountOverflow,
    #[error("InSufficient Balance")]
    InsufficientBalance,
    #[error("Destination Not Allowed")]
    DestinationNotAllowed,
    #[error("Balance Account Not Found")]
    BalanceAccountNotFound,
    #[error("Invalid Source Token Account")]
    InvalidSourceTokenAccount,
    #[error("Invalid Destination Token Account")]
    InvalidDestinationTokenAccount,
    #[error("Invalid Token Mint Account")]
    InvalidTokenMintAccount,
    #[error("Invalid Approval Timeout")]
    InvalidApprovalTimeout,
    #[error("Concurrent Operations Not Allowed")]
    ConcurrentOperationsNotAllowed,
    #[error("Simulation Finished Successfully")]
    SimulationFinished,
    #[error("Whitelist Is Disabled")]
    WhitelistDisabled,
    #[error("DApp Transactions Are Disabled")]
    DAppsDisabled,
}

impl From<WalletError> for ProgramError {
    fn from(e: WalletError) -> Self {
        ProgramError::Custom(e as u32)
    }
}
