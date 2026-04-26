//! Property-based tests for AmateRS SDK using `proptest`.
//!
//! Covers:
//! * `QueryBuilder` round-trip — arbitrary params produce valid `Query` structs.
//! * `SdkError` `Display` — every variant yields a non-empty message.
//! * Row constructor — arbitrary bytes are preserved exactly.
//!
//! Note: `proptest_codec_roundtrip` requires the `serialization` feature
//! (oxicode is feature-gated).  That test is conditionally compiled with
//! `#[cfg(feature = "serialization")]`.

use amaters_core::{CipherBlob, Key, Query, col};
use amaters_sdk_rust::{Row, SdkError, query};
use proptest::prelude::*;

// ---------------------------------------------------------------------------
// Helpers / strategies
// ---------------------------------------------------------------------------

/// Strategy producing arbitrary byte vectors up to 256 bytes.
fn arb_bytes() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..=256)
}

/// Strategy producing arbitrary non-empty ASCII strings suitable for
/// collection names and key strings.
fn arb_name() -> impl Strategy<Value = String> {
    // Use a restricted alphabet so the string is always valid UTF-8 and
    // never empty.
    "[a-zA-Z][a-zA-Z0-9_:]{0,63}".prop_map(|s| s)
}

// ---------------------------------------------------------------------------
// D2-P1: QueryBuilder round-trip
//
// Build queries with arbitrary parameters.  Verify:
//  * The builder does not panic.
//  * The resulting `Query` has the correct collection name.
//  * Key bytes survive the round-trip through `Key::from_slice`.
// ---------------------------------------------------------------------------

proptest! {
    #[test]
    fn proptest_query_builder_roundtrip(
        collection in arb_name(),
        key_bytes in arb_bytes(),
        value_bytes in arb_bytes(),
    ) {
        // --- Get ---
        let key = Key::from_slice(&key_bytes);
        let q = query(&collection).get(key.clone());
        match &q {
            Query::Get { collection: c, key: k } => {
                prop_assert_eq!(c, &collection);
                prop_assert_eq!(k.as_bytes(), key.as_bytes());
            }
            other => prop_assert!(false, "expected Get, got {:?}", other),
        }

        // --- Set ---
        let value = CipherBlob::new(value_bytes.clone());
        let q = query(&collection).set(key.clone(), value.clone());
        match &q {
            Query::Set { collection: c, key: k, value: v } => {
                prop_assert_eq!(c, &collection);
                prop_assert_eq!(k.as_bytes(), key.as_bytes());
                prop_assert_eq!(v.to_vec(), value_bytes);
            }
            other => prop_assert!(false, "expected Set, got {:?}", other),
        }

        // --- Delete ---
        let q = query(&collection).delete(key.clone());
        match &q {
            Query::Delete { collection: c, key: k } => {
                prop_assert_eq!(c, &collection);
                prop_assert_eq!(k.as_bytes(), key.as_bytes());
            }
            other => prop_assert!(false, "expected Delete, got {:?}", other),
        }

        // --- Range ---
        let start = Key::from_slice(&key_bytes);
        let end_bytes: Vec<u8> = key_bytes.iter().map(|b| b.saturating_add(1)).collect();
        let end = Key::from_slice(&end_bytes);
        let q = query(&collection).range(start.clone(), end.clone());
        match &q {
            Query::Range { collection: c, start: s, end: e } => {
                prop_assert_eq!(c, &collection);
                prop_assert_eq!(s.as_bytes(), start.as_bytes());
                prop_assert_eq!(e.as_bytes(), end.as_bytes());
            }
            other => prop_assert!(false, "expected Range, got {:?}", other),
        }
    }
}

// ---------------------------------------------------------------------------
// D2-P2: SdkError Display is always non-empty
//
// For every variant we can construct without external resources, verify that
// `format!("{}", err)` produces a non-empty string.
// ---------------------------------------------------------------------------

proptest! {
    #[test]
    fn proptest_error_display_not_empty(msg in ".*") {
        let variants: &[SdkError] = &[
            SdkError::Connection(msg.clone()),
            SdkError::Timeout(msg.clone()),
            SdkError::Configuration(msg.clone()),
            SdkError::Serialization(msg.clone()),
            SdkError::Fhe(msg.clone()),
            SdkError::InvalidArgument(msg.clone()),
            SdkError::NotFound(msg.clone()),
            SdkError::OperationFailed(msg.clone()),
            SdkError::Other(msg.clone()),
        ];

        for err in variants {
            let display = format!("{err}");
            prop_assert!(
                !display.is_empty(),
                "Display output for {:?} was empty",
                err
            );
        }
    }
}

// ---------------------------------------------------------------------------
// D2-P3: Row constructor byte round-trip
//
// Verify that arbitrary key/value bytes fed to `Row::new` are stored exactly
// without modification.
// ---------------------------------------------------------------------------

proptest! {
    #[test]
    fn proptest_row_bytes_roundtrip(
        key_bytes in arb_bytes(),
        value_bytes in arb_bytes(),
    ) {
        let row = Row::new(key_bytes.clone(), value_bytes.clone());
        prop_assert_eq!(&row.key, &key_bytes, "key bytes should be preserved");
        prop_assert_eq!(&row.value, &value_bytes, "value bytes should be preserved");
    }
}

// ---------------------------------------------------------------------------
// D2-P4: Filter builder — arbitrary predicates produce Filter queries
// ---------------------------------------------------------------------------

proptest! {
    #[test]
    fn proptest_filter_builder_roundtrip(
        collection in arb_name(),
        col_name in "[a-z][a-z_]{0,15}",
        value_bytes in arb_bytes(),
    ) {
        let value = CipherBlob::new(value_bytes);
        let q = query(&collection)
            .where_clause()
            .eq(col(&col_name), value)
            .build();

        match &q {
            Query::Filter { collection: c, predicate } => {
                prop_assert_eq!(c, &collection);
                prop_assert!(
                    matches!(predicate, amaters_core::Predicate::Eq(_, _)),
                    "expected Eq predicate"
                );
            }
            other => prop_assert!(false, "expected Filter, got {:?}", other),
        }
    }
}

// ---------------------------------------------------------------------------
// D2-P5: codec round-trip (serialization feature only)
//
// If the `serialization` feature is active (oxicode available), verify that
// encoding a serializable value and decoding it yields the same bytes.
//
// We encode a `Vec<u8>` directly (which always implements serde) rather than
// `CipherBlob` (whose serde impl is also feature-gated in amaters-core and
// is not guaranteed to be active in this test binary).
// ---------------------------------------------------------------------------

#[cfg(feature = "serialization")]
proptest! {
    #[test]
    fn proptest_codec_roundtrip(data in arb_bytes()) {
        use oxicode::serde::{decode_serde, encode_serde};

        // Encode a plain Vec<u8> — it always implements serde.
        let encoded: Vec<u8> = encode_serde(&data)
            .expect("encode should succeed");
        let decoded: Vec<u8> = decode_serde(&encoded)
            .expect("decode should succeed");

        prop_assert_eq!(decoded, data, "round-trip should preserve bytes");
    }
}
