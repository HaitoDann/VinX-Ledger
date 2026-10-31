//! Tout message venu d'un pair (consensus, synchronisation, instantanés) est
//! décodé sans planter, et seul l'encodage canonique est accepté.
#![no_main]

use borsh::{BorshDeserialize, BorshSerialize};
use libfuzzer_sys::fuzz_target;
use vinx_core::{Block, CommitCertificate, SignedAdminTransaction, SignedTransaction};
use vinx_node::network::{NetMessage, SyncRequest, SyncResponse};

fn canonical<T: BorshDeserialize + BorshSerialize>(bytes: &[u8]) {
    if let Ok(v) = borsh::from_slice::<T>(bytes) {
        assert_eq!(borsh::to_vec(&v).unwrap(), bytes, "encodage non canonique accepté");
    }
}

fuzz_target!(|bytes: &[u8]| {
    canonical::<NetMessage>(bytes);
    canonical::<SyncRequest>(bytes);
    canonical::<SyncResponse>(bytes);
    canonical::<Block>(bytes);
    canonical::<CommitCertificate>(bytes);
    canonical::<SignedTransaction>(bytes);
    canonical::<SignedAdminTransaction>(bytes);
});
