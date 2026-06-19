#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Try to interpret data as a circuit node operation sequence.
    // No panics should occur from arbitrary byte input.
    if data.len() >= 2 {
        let op_byte = data[0] % 4;
        let val_byte = data[1];
        // Validate that basic circuit value construction is panic-free.
        let _circuit_val = match op_byte {
            0 => amaters_core::compute::CircuitValue::Bool(val_byte != 0),
            1 => amaters_core::compute::CircuitValue::U8(val_byte),
            2 => amaters_core::compute::CircuitValue::U16(val_byte as u16),
            _ => amaters_core::compute::CircuitValue::U32(val_byte as u32),
        };
    }
});
