use {
    solana_program_test::BanksClient,
    solana_sdk::{
        hash::Hash,
        signature::{Keypair, Signer},
        system_instruction,
        transaction::Transaction,
        transport::TransportError,
    },
    solana_program::{
        program_pack::Pack,
        pubkey::Pubkey,
    },
    strike_wallet::{
        instruction::{
            program_init,
        }, model::program_config::ProgramConfig,
    },
};

pub async fn init_program(
    banks_client: &mut BanksClient,
    payer: &Keypair,
    recent_blockhash: Hash,
    program_owner: &Keypair,
    program_config_account: &Keypair,
    assistant_account: &Keypair,
    approvals_required_for_config: Option<u8>,
    config_approvers: Option<Vec<Pubkey>>,
) -> Result<(), TransportError> {
    let rent = banks_client.get_rent().await.unwrap();
    let program_rent = rent.minimum_balance(ProgramConfig::LEN);

    let transaction = Transaction::new_signed_with_payer(
        &[
            system_instruction::create_account(
                &payer.pubkey(),
                &program_config_account.pubkey(),
                program_rent,
                ProgramConfig::LEN as u64,
                &program_owner.pubkey(),
            ),
            program_init(
                &program_owner.pubkey(),
                &program_config_account.pubkey(),
                &assistant_account.pubkey(),
                config_approvers.unwrap_or(Vec::new()),
                approvals_required_for_config.unwrap_or(0),
            ).unwrap(),],
        Some(&payer.pubkey()),
        &[payer, program_config_account, assistant_account],
        recent_blockhash,
    );
    banks_client.process_transaction(transaction).await?;
    Ok(())
}
