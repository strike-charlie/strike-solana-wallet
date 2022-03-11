// #![cfg(feature = "test-bpf")]

use solana_program_test::tokio;
use solana_sdk::signature::{Keypair, Signer};
use strike_wallet::constants::GUID_HASH_BYTES;
use strike_wallet::instruction::{
    pack_balance_account_guid_hash_vec, unpack_account_guid_hash_vec,
};
use strike_wallet::model::balance_account::BalanceAccountGuidHash;

fn build_account_guid_hash_byte_vec(n: u8) -> (Vec<BalanceAccountGuidHash>, Vec<u8>) {
    let mut buf = Vec::<u8>::new();
    let mut account_guid_hashes = Vec::<BalanceAccountGuidHash>::with_capacity(n as usize);
    for _ in 0..n {
        account_guid_hashes.push(BalanceAccountGuidHash::new(
            &Keypair::new().pubkey().to_bytes(),
        ));
    }
    pack_balance_account_guid_hash_vec(&account_guid_hashes, &mut buf);
    (account_guid_hashes, buf)
}

#[tokio::test]
async fn test_pack_account_guid_hash_vec() {
    // create vec of 2 GUID hashes.
    let n: u8 = 2;
    let (hashes, buf) = build_account_guid_hash_byte_vec(n);

    // verify first byte for vec len
    assert_eq!(buf[0], n);

    // verify that each following 32 byte segment is the corresponding
    // GUID hash bytes.
    for i in 0..(n as usize) {
        assert_eq!(
            buf[1 + i * GUID_HASH_BYTES..1 + (i + 1) * GUID_HASH_BYTES],
            *hashes[i].to_bytes()
        );
    }
}

#[tokio::test]
async fn test_unpack_account_guid_hash_vec() {
    let (exp_hashes, buf) = build_account_guid_hash_byte_vec(2);
    let hashes = unpack_account_guid_hash_vec(&buf).unwrap();

    exp_hashes
        .iter()
        .zip(hashes.iter())
        .for_each(|(exp_hash, hash)| {
            assert_eq!(*hash, *exp_hash);
        });
}
