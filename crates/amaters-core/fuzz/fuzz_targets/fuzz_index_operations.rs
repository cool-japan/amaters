#![no_main]
use amaters_core::storage::EncryptedIndex;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let index = EncryptedIndex::new("fuzz", "col", "field");
    if data.len() >= 9 {
        let op = data[0];
        let key = &data[1..5];
        let record_id_bytes: [u8; 8] = match data[1..9].try_into() {
            Ok(b) => b,
            Err(_) => return,
        };
        let record_id = u64::from_le_bytes(record_id_bytes);
        match op % 3 {
            0 => index.insert(key, record_id),
            1 => index.remove(key, record_id),
            _ => {
                let _ = index.lookup_candidates(key);
            }
        }
    }
});
