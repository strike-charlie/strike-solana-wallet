use crate::model::address_book::DAppBookEntry;
use crate::model::balance_account::BalanceAccountGuidHash;
use arrayref::{array_mut_ref, array_ref, array_refs, mut_array_refs};
use solana_program::entrypoint::ProgramResult;
use solana_program::program_error::ProgramError;
use solana_program::program_pack::{IsInitialized, Pack, Sealed};
use solana_program::pubkey::{Pubkey, PUBKEY_BYTES};

const INSTRUCTION_DATA_LEN: usize = 2500;
const MAX_INSTRUCTION_COUNT: usize = 32;

#[derive(Debug)]
pub struct DAppMultisigData {
    pub is_initialized: bool,
    pub wallet_address: Pubkey,
    pub account_guid_hash: BalanceAccountGuidHash,
    pub dapp: DAppBookEntry,
    pub num_instructions: u16,
    instruction_offsets: [u16; MAX_INSTRUCTION_COUNT],
    instruction_data: Vec<u8>,
}

impl DAppMultisigData {
    pub fn init(
        &mut self,
        wallet_address: Pubkey,
        account_guid_hash: BalanceAccountGuidHash,
        dapp: DAppBookEntry,
        num_instructions: u16,
    ) -> ProgramResult {
        self.is_initialized = true;
        self.wallet_address = wallet_address;
        self.account_guid_hash = account_guid_hash;
        self.dapp = dapp;
        if num_instructions > MAX_INSTRUCTION_COUNT as u16 {
            panic!("Too many instructions")
        }
        self.num_instructions = num_instructions;
        self.instruction_offsets = [0; MAX_INSTRUCTION_COUNT];
        self.instruction_data = vec![0; INSTRUCTION_DATA_LEN];

        Ok(())
    }
}

impl Sealed for DAppMultisigData {}

impl IsInitialized for DAppMultisigData {
    fn is_initialized(&self) -> bool {
        self.is_initialized
    }
}

impl Pack for DAppMultisigData {
    const LEN: usize = 1
        + PUBKEY_BYTES
        + 32
        + DAppBookEntry::LEN
        + 2
        + 2 * MAX_INSTRUCTION_COUNT
        + INSTRUCTION_DATA_LEN;

    fn pack_into_slice(&self, dst: &mut [u8]) {
        let dst = array_mut_ref![dst, 0, DAppMultisigData::LEN];
        let (
            is_initialized_dst,
            wallet_address_dst,
            account_guid_hash_dst,
            dapp_dst,
            num_instructions_dst,
            instruction_offsets_dst,
            instruction_data_dst,
        ) = mut_array_refs![
            dst,
            1,
            PUBKEY_BYTES,
            32,
            DAppBookEntry::LEN,
            2,
            2 * MAX_INSTRUCTION_COUNT,
            INSTRUCTION_DATA_LEN
        ];

        let DAppMultisigData {
            is_initialized,
            wallet_address,
            account_guid_hash,
            dapp,
            num_instructions,
            instruction_offsets,
            instruction_data,
        } = self;

        is_initialized_dst[0] = *is_initialized as u8;
        *wallet_address_dst = wallet_address.to_bytes();
        account_guid_hash_dst.copy_from_slice(account_guid_hash.to_bytes());
        dapp.pack_into_slice(dapp_dst);
        *num_instructions_dst = num_instructions.to_le_bytes();
        instruction_offsets_dst
            .chunks_exact_mut(2)
            .take(MAX_INSTRUCTION_COUNT)
            .enumerate()
            .for_each(|(i, chunk)| {
                chunk[0] = (instruction_offsets[i] & 0xFF) as u8;
                chunk[1] = ((instruction_offsets[i] & 0xFF00) >> 8) as u8;
            });
        instruction_data_dst.copy_from_slice(instruction_data);
    }

    fn unpack_from_slice(src: &[u8]) -> Result<Self, ProgramError> {
        let src = array_ref![src, 0, DAppMultisigData::LEN];
        let (
            is_initialized,
            wallet_address,
            account_guid_hash,
            dapp,
            num_instructions,
            instruction_offsets,
            instruction_data,
        ) = array_refs![
            src,
            1,
            PUBKEY_BYTES,
            32,
            DAppBookEntry::LEN,
            2,
            2 * MAX_INSTRUCTION_COUNT,
            INSTRUCTION_DATA_LEN
        ];

        let is_initialized = match is_initialized {
            [0] => false,
            [1] => true,
            _ => return Err(ProgramError::InvalidAccountData),
        };

        let mut instruction_offsets_array: [u16; MAX_INSTRUCTION_COUNT] =
            [0; MAX_INSTRUCTION_COUNT];

        instruction_offsets
            .chunks_exact(2)
            .enumerate()
            .for_each(|(i, chunk)| {
                instruction_offsets_array[i] = u16::from_le_bytes([chunk[0], chunk[1]])
            });

        let wallet_address = Pubkey::new_from_array(*wallet_address);
        let account_guid_hash = BalanceAccountGuidHash::new(account_guid_hash);
        let dapp = DAppBookEntry::unpack_from_slice(dapp).unwrap();
        let num_instructions = u16::from_le_bytes(*num_instructions);

        Ok(DAppMultisigData {
            is_initialized,
            wallet_address,
            account_guid_hash,
            dapp,
            num_instructions,
            instruction_offsets: instruction_offsets_array,
            instruction_data: instruction_data[..].to_owned(),
        })
    }
}

#[cfg(test)]
mod test {
    use crate::model::address_book::{DAppBookEntry, DAppBookEntryNameHash};
    use crate::model::balance_account::BalanceAccountGuidHash;
    use crate::model::dapp_multisig_data::{DAppMultisigData, INSTRUCTION_DATA_LEN};
    use arrayref::array_ref;
    use sha2::Digest;
    use sha2::Sha256;
    use solana_program::program_pack::Pack;
    use solana_program::pubkey::Pubkey;

    fn hash_of(data: &[u8]) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let hash_output = hasher.finalize();
        *array_ref![hash_output, 0, 32]
    }

    #[test]
    fn test_pack_unpack_empty() {
        let data = DAppMultisigData {
            is_initialized: false,
            wallet_address: Pubkey::new(&[0; 32]),
            account_guid_hash: BalanceAccountGuidHash::new(&[0; 32]),
            dapp: DAppBookEntry {
                address: Pubkey::new(&[0; 32]),
                name_hash: DAppBookEntryNameHash::new(&[0; 32]),
            },
            num_instructions: 0,
            instruction_offsets: [0; 32],
            instruction_data: vec![0; INSTRUCTION_DATA_LEN],
        };
        let mut buffer = vec![0; DAppMultisigData::LEN];
        data.pack_into_slice(&mut buffer);
        let data2 = DAppMultisigData::unpack_from_slice(&buffer).unwrap();
        compare_data(data, data2);
    }

    #[test]
    fn test_pack_unpack_initialized() {
        let data = DAppMultisigData {
            is_initialized: true,
            wallet_address: Pubkey::new_unique(),
            account_guid_hash: BalanceAccountGuidHash::new(&hash_of(b"account-guid")),
            dapp: DAppBookEntry {
                address: Pubkey::new_unique(),
                name_hash: DAppBookEntryNameHash::new(&hash_of(b"dapp-name")),
            },
            num_instructions: 3,
            instruction_offsets: [
                1, 2, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0,
            ],
            instruction_data: vec![1; INSTRUCTION_DATA_LEN],
        };
        let mut buffer = vec![0; DAppMultisigData::LEN];
        data.pack_into_slice(&mut buffer);
        let data2 = DAppMultisigData::unpack_from_slice(&buffer).unwrap();
        compare_data(data, data2);
    }

    fn compare_data(data: DAppMultisigData, data2: DAppMultisigData) {
        assert_eq!(data.is_initialized, data2.is_initialized);
        assert_eq!(
            data.wallet_address.to_bytes(),
            data2.wallet_address.to_bytes()
        );
        assert_eq!(
            data.account_guid_hash.to_bytes(),
            data2.account_guid_hash.to_bytes()
        );
        assert_eq!(data.dapp.address.to_bytes(), data2.dapp.address.to_bytes());
        assert_eq!(
            data.dapp.name_hash.to_bytes(),
            data2.dapp.name_hash.to_bytes()
        );
        assert_eq!(data.num_instructions, data2.num_instructions);
        assert_eq!(data.instruction_offsets, data2.instruction_offsets);
        assert_eq!(data.instruction_data, data2.instruction_data);
    }
}
