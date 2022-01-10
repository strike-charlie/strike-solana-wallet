#![cfg(feature = "test-bpf")]

mod action;

use solana_sdk::account::ReadableAccount;
use {
    solana_program_test::{processor, tokio, ProgramTest},
    solana_sdk::{
        program_pack::Pack,
        pubkey::Pubkey,
        signature::{Keypair, Signer},
    },
    strike_wallet::{
        processor::Processor,
        model::program_config::ProgramConfig,
    },
};

#[tokio::test]
async fn init_program() {
    let program_owner = Keypair::new();
    let mut pt = ProgramTest::new("strike_wallet", program_owner.pubkey(), processor!(Processor::process));
    pt.set_bpf_compute_max_units(5_000);
    let (mut banks_client, payer, recent_blockhash) = pt.start().await;
    let program_config_account = Keypair::new();
    let assistant_account = Keypair::new();

    action::init_program(
        &mut banks_client,
        &payer,
        recent_blockhash,
        &program_owner,
        &program_config_account,
        &assistant_account,
        None,
        None,
    )
    .await
    .unwrap();

    let config = ProgramConfig::unpack_from_slice(
        banks_client.get_account(program_config_account.pubkey()).await.unwrap().unwrap().data()
    ).unwrap();
    assert_eq!(config.approvals_required_for_config, 0);
    assert_eq!(config.config_approvers, Vec::new());
}
