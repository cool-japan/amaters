//! FHE value type with arithmetic, comparison, and boolean operations
//!
//! This module exposes client-side homomorphic operations on encrypted values.
//! All operations require the `fhe` feature and a server key set via
//! [`FheKeys::set_as_global_server_key`].

use crate::error::{Result, SdkError};

#[cfg(feature = "fhe")]
use crate::fhe::FheKeys;

/// The kind of value held inside an [`FheValue`]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FheValueKind {
    Bool,
    U8,
    U16,
    U32,
    U64,
}

impl std::fmt::Display for FheValueKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FheValueKind::Bool => write!(f, "Bool"),
            FheValueKind::U8 => write!(f, "U8"),
            FheValueKind::U16 => write!(f, "U16"),
            FheValueKind::U32 => write!(f, "U32"),
            FheValueKind::U64 => write!(f, "U64"),
        }
    }
}

/// Inner encrypted value variants (only present when `fhe` feature is enabled)
#[cfg(feature = "fhe")]
enum FheValueInner {
    Bool(tfhe::FheBool),
    U8(tfhe::FheUint8),
    U16(tfhe::FheUint16),
    U32(tfhe::FheUint32),
    U64(tfhe::FheUint64),
}

/// An encrypted value that supports homomorphic operations
///
/// Wrap plaintext with `encrypt_*` constructors, then use `add`/`sub`/`mul`,
/// comparison methods (`eq`/`ne`/`lt`/`le`/`gt`/`ge`), and boolean methods
/// (`and`/`or`/`xor`/`not`) — all without decrypting.
///
/// # Requirements
///
/// Before calling any operation, you must call [`FheKeys::set_as_global_server_key`].
/// The `fhe` feature must be enabled.
pub struct FheValue {
    #[cfg(feature = "fhe")]
    inner: FheValueInner,
    #[cfg(not(feature = "fhe"))]
    _placeholder: (),
}

impl FheValue {
    /// Return the type of this encrypted value
    pub fn kind(&self) -> FheValueKind {
        #[cfg(feature = "fhe")]
        {
            match &self.inner {
                FheValueInner::Bool(_) => FheValueKind::Bool,
                FheValueInner::U8(_) => FheValueKind::U8,
                FheValueInner::U16(_) => FheValueKind::U16,
                FheValueInner::U32(_) => FheValueKind::U32,
                FheValueInner::U64(_) => FheValueKind::U64,
            }
        }
        #[cfg(not(feature = "fhe"))]
        {
            FheValueKind::Bool
        }
    }
}

#[cfg(not(feature = "fhe"))]
impl FheValue {
    /// Stub: encrypt a boolean (requires `fhe` feature)
    pub fn encrypt_bool(_val: bool, _keys: &crate::fhe::FheKeys) -> Result<Self> {
        Err(SdkError::Fhe("fhe feature not enabled".to_string()))
    }

    /// Stub: encrypt a u8 (requires `fhe` feature)
    pub fn encrypt_u8(_val: u8, _keys: &crate::fhe::FheKeys) -> Result<Self> {
        Err(SdkError::Fhe("fhe feature not enabled".to_string()))
    }

    /// Stub: encrypt a u16 (requires `fhe` feature)
    pub fn encrypt_u16(_val: u16, _keys: &crate::fhe::FheKeys) -> Result<Self> {
        Err(SdkError::Fhe("fhe feature not enabled".to_string()))
    }

    /// Stub: encrypt a u32 (requires `fhe` feature)
    pub fn encrypt_u32(_val: u32, _keys: &crate::fhe::FheKeys) -> Result<Self> {
        Err(SdkError::Fhe("fhe feature not enabled".to_string()))
    }

    /// Stub: encrypt a u64 (requires `fhe` feature)
    pub fn encrypt_u64(_val: u64, _keys: &crate::fhe::FheKeys) -> Result<Self> {
        Err(SdkError::Fhe("fhe feature not enabled".to_string()))
    }

    /// Stub: add two values (requires `fhe` feature)
    pub fn add(&self, _other: &Self) -> Result<Self> {
        Err(SdkError::Fhe("fhe feature not enabled".to_string()))
    }

    /// Stub: subtract two values (requires `fhe` feature)
    pub fn sub(&self, _other: &Self) -> Result<Self> {
        Err(SdkError::Fhe("fhe feature not enabled".to_string()))
    }

    /// Stub: multiply two values (requires `fhe` feature)
    pub fn mul(&self, _other: &Self) -> Result<Self> {
        Err(SdkError::Fhe("fhe feature not enabled".to_string()))
    }

    /// Stub: compare equality (requires `fhe` feature)
    pub fn eq(&self, _other: &Self) -> Result<Self> {
        Err(SdkError::Fhe("fhe feature not enabled".to_string()))
    }

    /// Stub: compare not-equal (requires `fhe` feature)
    pub fn ne(&self, _other: &Self) -> Result<Self> {
        Err(SdkError::Fhe("fhe feature not enabled".to_string()))
    }

    /// Stub: compare less-than (requires `fhe` feature)
    pub fn lt(&self, _other: &Self) -> Result<Self> {
        Err(SdkError::Fhe("fhe feature not enabled".to_string()))
    }

    /// Stub: compare less-than-or-equal (requires `fhe` feature)
    pub fn le(&self, _other: &Self) -> Result<Self> {
        Err(SdkError::Fhe("fhe feature not enabled".to_string()))
    }

    /// Stub: compare greater-than (requires `fhe` feature)
    pub fn gt(&self, _other: &Self) -> Result<Self> {
        Err(SdkError::Fhe("fhe feature not enabled".to_string()))
    }

    /// Stub: compare greater-than-or-equal (requires `fhe` feature)
    pub fn ge(&self, _other: &Self) -> Result<Self> {
        Err(SdkError::Fhe("fhe feature not enabled".to_string()))
    }

    /// Stub: boolean AND (requires `fhe` feature)
    pub fn and(&self, _other: &Self) -> Result<Self> {
        Err(SdkError::Fhe("fhe feature not enabled".to_string()))
    }

    /// Stub: boolean OR (requires `fhe` feature)
    pub fn or(&self, _other: &Self) -> Result<Self> {
        Err(SdkError::Fhe("fhe feature not enabled".to_string()))
    }

    /// Stub: boolean XOR (requires `fhe` feature)
    pub fn xor(&self, _other: &Self) -> Result<Self> {
        Err(SdkError::Fhe("fhe feature not enabled".to_string()))
    }

    /// Stub: boolean NOT (requires `fhe` feature)
    pub fn not(&self) -> Result<Self> {
        Err(SdkError::Fhe("fhe feature not enabled".to_string()))
    }

    /// Stub: decrypt bool (requires `fhe` feature)
    pub fn decrypt_bool(&self, _keys: &crate::fhe::FheKeys) -> Result<bool> {
        Err(SdkError::Fhe("fhe feature not enabled".to_string()))
    }

    /// Stub: decrypt u8 (requires `fhe` feature)
    pub fn decrypt_u8(&self, _keys: &crate::fhe::FheKeys) -> Result<u8> {
        Err(SdkError::Fhe("fhe feature not enabled".to_string()))
    }

    /// Stub: decrypt u16 (requires `fhe` feature)
    pub fn decrypt_u16(&self, _keys: &crate::fhe::FheKeys) -> Result<u16> {
        Err(SdkError::Fhe("fhe feature not enabled".to_string()))
    }

    /// Stub: decrypt u32 (requires `fhe` feature)
    pub fn decrypt_u32(&self, _keys: &crate::fhe::FheKeys) -> Result<u32> {
        Err(SdkError::Fhe("fhe feature not enabled".to_string()))
    }

    /// Stub: decrypt u64 (requires `fhe` feature)
    pub fn decrypt_u64(&self, _keys: &crate::fhe::FheKeys) -> Result<u64> {
        Err(SdkError::Fhe("fhe feature not enabled".to_string()))
    }
}

#[cfg(feature = "fhe")]
impl FheValue {
    /// Encrypt a boolean value
    pub fn encrypt_bool(val: bool, keys: &FheKeys) -> Result<Self> {
        use tfhe::prelude::FheEncrypt;
        let encrypted = tfhe::FheBool::encrypt(val, keys.client_key());
        Ok(Self {
            inner: FheValueInner::Bool(encrypted),
        })
    }

    /// Encrypt a u8 value
    pub fn encrypt_u8(val: u8, keys: &FheKeys) -> Result<Self> {
        use tfhe::prelude::FheEncrypt;
        let encrypted = tfhe::FheUint8::encrypt(val, keys.client_key());
        Ok(Self {
            inner: FheValueInner::U8(encrypted),
        })
    }

    /// Encrypt a u16 value
    pub fn encrypt_u16(val: u16, keys: &FheKeys) -> Result<Self> {
        use tfhe::prelude::FheEncrypt;
        let encrypted = tfhe::FheUint16::encrypt(val, keys.client_key());
        Ok(Self {
            inner: FheValueInner::U16(encrypted),
        })
    }

    /// Encrypt a u32 value
    pub fn encrypt_u32(val: u32, keys: &FheKeys) -> Result<Self> {
        use tfhe::prelude::FheEncrypt;
        let encrypted = tfhe::FheUint32::encrypt(val, keys.client_key());
        Ok(Self {
            inner: FheValueInner::U32(encrypted),
        })
    }

    /// Encrypt a u64 value
    pub fn encrypt_u64(val: u64, keys: &FheKeys) -> Result<Self> {
        use tfhe::prelude::FheEncrypt;
        let encrypted = tfhe::FheUint64::encrypt(val, keys.client_key());
        Ok(Self {
            inner: FheValueInner::U64(encrypted),
        })
    }

    /// Decrypt to bool (only valid for Bool variant)
    pub fn decrypt_bool(&self, keys: &FheKeys) -> Result<bool> {
        use tfhe::prelude::FheDecrypt;
        match &self.inner {
            FheValueInner::Bool(v) => Ok(v.decrypt(keys.client_key())),
            _ => Err(SdkError::Fhe(format!(
                "type mismatch: expected Bool, got {}",
                self.kind()
            ))),
        }
    }

    /// Decrypt to u8 (only valid for U8 variant)
    pub fn decrypt_u8(&self, keys: &FheKeys) -> Result<u8> {
        use tfhe::prelude::FheDecrypt;
        match &self.inner {
            FheValueInner::U8(v) => Ok(v.decrypt(keys.client_key())),
            _ => Err(SdkError::Fhe(format!(
                "type mismatch: expected U8, got {}",
                self.kind()
            ))),
        }
    }

    /// Decrypt to u16 (only valid for U16 variant)
    pub fn decrypt_u16(&self, keys: &FheKeys) -> Result<u16> {
        use tfhe::prelude::FheDecrypt;
        match &self.inner {
            FheValueInner::U16(v) => Ok(v.decrypt(keys.client_key())),
            _ => Err(SdkError::Fhe(format!(
                "type mismatch: expected U16, got {}",
                self.kind()
            ))),
        }
    }

    /// Decrypt to u32 (only valid for U32 variant)
    pub fn decrypt_u32(&self, keys: &FheKeys) -> Result<u32> {
        use tfhe::prelude::FheDecrypt;
        match &self.inner {
            FheValueInner::U32(v) => Ok(v.decrypt(keys.client_key())),
            _ => Err(SdkError::Fhe(format!(
                "type mismatch: expected U32, got {}",
                self.kind()
            ))),
        }
    }

    /// Decrypt to u64 (only valid for U64 variant)
    pub fn decrypt_u64(&self, keys: &FheKeys) -> Result<u64> {
        use tfhe::prelude::FheDecrypt;
        match &self.inner {
            FheValueInner::U64(v) => Ok(v.decrypt(keys.client_key())),
            _ => Err(SdkError::Fhe(format!(
                "type mismatch: expected U64, got {}",
                self.kind()
            ))),
        }
    }

    /// Add two encrypted integers (same type required)
    pub fn add(&self, other: &Self) -> Result<Self> {
        match (&self.inner, &other.inner) {
            (FheValueInner::U8(a), FheValueInner::U8(b)) => Ok(Self {
                inner: FheValueInner::U8(a + b),
            }),
            (FheValueInner::U16(a), FheValueInner::U16(b)) => Ok(Self {
                inner: FheValueInner::U16(a + b),
            }),
            (FheValueInner::U32(a), FheValueInner::U32(b)) => Ok(Self {
                inner: FheValueInner::U32(a + b),
            }),
            (FheValueInner::U64(a), FheValueInner::U64(b)) => Ok(Self {
                inner: FheValueInner::U64(a + b),
            }),
            _ => Err(SdkError::Fhe(format!(
                "type mismatch in add: {} vs {}",
                self.kind(),
                other.kind()
            ))),
        }
    }

    /// Subtract two encrypted integers (same type required)
    pub fn sub(&self, other: &Self) -> Result<Self> {
        match (&self.inner, &other.inner) {
            (FheValueInner::U8(a), FheValueInner::U8(b)) => Ok(Self {
                inner: FheValueInner::U8(a - b),
            }),
            (FheValueInner::U16(a), FheValueInner::U16(b)) => Ok(Self {
                inner: FheValueInner::U16(a - b),
            }),
            (FheValueInner::U32(a), FheValueInner::U32(b)) => Ok(Self {
                inner: FheValueInner::U32(a - b),
            }),
            (FheValueInner::U64(a), FheValueInner::U64(b)) => Ok(Self {
                inner: FheValueInner::U64(a - b),
            }),
            _ => Err(SdkError::Fhe(format!(
                "type mismatch in sub: {} vs {}",
                self.kind(),
                other.kind()
            ))),
        }
    }

    /// Multiply two encrypted integers (same type required)
    pub fn mul(&self, other: &Self) -> Result<Self> {
        match (&self.inner, &other.inner) {
            (FheValueInner::U8(a), FheValueInner::U8(b)) => Ok(Self {
                inner: FheValueInner::U8(a * b),
            }),
            (FheValueInner::U16(a), FheValueInner::U16(b)) => Ok(Self {
                inner: FheValueInner::U16(a * b),
            }),
            (FheValueInner::U32(a), FheValueInner::U32(b)) => Ok(Self {
                inner: FheValueInner::U32(a * b),
            }),
            (FheValueInner::U64(a), FheValueInner::U64(b)) => Ok(Self {
                inner: FheValueInner::U64(a * b),
            }),
            _ => Err(SdkError::Fhe(format!(
                "type mismatch in mul: {} vs {}",
                self.kind(),
                other.kind()
            ))),
        }
    }

    /// Compare equality — returns a Bool-variant FheValue
    pub fn eq(&self, other: &Self) -> Result<Self> {
        use tfhe::prelude::FheEq;
        match (&self.inner, &other.inner) {
            (FheValueInner::U8(a), FheValueInner::U8(b)) => Ok(Self {
                inner: FheValueInner::Bool(a.eq(b)),
            }),
            (FheValueInner::U16(a), FheValueInner::U16(b)) => Ok(Self {
                inner: FheValueInner::Bool(a.eq(b)),
            }),
            (FheValueInner::U32(a), FheValueInner::U32(b)) => Ok(Self {
                inner: FheValueInner::Bool(a.eq(b)),
            }),
            (FheValueInner::U64(a), FheValueInner::U64(b)) => Ok(Self {
                inner: FheValueInner::Bool(a.eq(b)),
            }),
            _ => Err(SdkError::Fhe(format!(
                "type mismatch in eq: {} vs {}",
                self.kind(),
                other.kind()
            ))),
        }
    }

    /// Compare not-equal — returns a Bool-variant FheValue
    pub fn ne(&self, other: &Self) -> Result<Self> {
        use tfhe::prelude::FheEq;
        match (&self.inner, &other.inner) {
            (FheValueInner::U8(a), FheValueInner::U8(b)) => Ok(Self {
                inner: FheValueInner::Bool(a.ne(b)),
            }),
            (FheValueInner::U16(a), FheValueInner::U16(b)) => Ok(Self {
                inner: FheValueInner::Bool(a.ne(b)),
            }),
            (FheValueInner::U32(a), FheValueInner::U32(b)) => Ok(Self {
                inner: FheValueInner::Bool(a.ne(b)),
            }),
            (FheValueInner::U64(a), FheValueInner::U64(b)) => Ok(Self {
                inner: FheValueInner::Bool(a.ne(b)),
            }),
            _ => Err(SdkError::Fhe(format!(
                "type mismatch in ne: {} vs {}",
                self.kind(),
                other.kind()
            ))),
        }
    }

    /// Compare less-than — returns a Bool-variant FheValue
    pub fn lt(&self, other: &Self) -> Result<Self> {
        use tfhe::prelude::FheOrd;
        match (&self.inner, &other.inner) {
            (FheValueInner::U8(a), FheValueInner::U8(b)) => Ok(Self {
                inner: FheValueInner::Bool(a.lt(b)),
            }),
            (FheValueInner::U16(a), FheValueInner::U16(b)) => Ok(Self {
                inner: FheValueInner::Bool(a.lt(b)),
            }),
            (FheValueInner::U32(a), FheValueInner::U32(b)) => Ok(Self {
                inner: FheValueInner::Bool(a.lt(b)),
            }),
            (FheValueInner::U64(a), FheValueInner::U64(b)) => Ok(Self {
                inner: FheValueInner::Bool(a.lt(b)),
            }),
            _ => Err(SdkError::Fhe(format!(
                "type mismatch in lt: {} vs {}",
                self.kind(),
                other.kind()
            ))),
        }
    }

    /// Compare less-than-or-equal — returns a Bool-variant FheValue
    pub fn le(&self, other: &Self) -> Result<Self> {
        use tfhe::prelude::FheOrd;
        match (&self.inner, &other.inner) {
            (FheValueInner::U8(a), FheValueInner::U8(b)) => Ok(Self {
                inner: FheValueInner::Bool(a.le(b)),
            }),
            (FheValueInner::U16(a), FheValueInner::U16(b)) => Ok(Self {
                inner: FheValueInner::Bool(a.le(b)),
            }),
            (FheValueInner::U32(a), FheValueInner::U32(b)) => Ok(Self {
                inner: FheValueInner::Bool(a.le(b)),
            }),
            (FheValueInner::U64(a), FheValueInner::U64(b)) => Ok(Self {
                inner: FheValueInner::Bool(a.le(b)),
            }),
            _ => Err(SdkError::Fhe(format!(
                "type mismatch in le: {} vs {}",
                self.kind(),
                other.kind()
            ))),
        }
    }

    /// Compare greater-than — returns a Bool-variant FheValue
    pub fn gt(&self, other: &Self) -> Result<Self> {
        use tfhe::prelude::FheOrd;
        match (&self.inner, &other.inner) {
            (FheValueInner::U8(a), FheValueInner::U8(b)) => Ok(Self {
                inner: FheValueInner::Bool(a.gt(b)),
            }),
            (FheValueInner::U16(a), FheValueInner::U16(b)) => Ok(Self {
                inner: FheValueInner::Bool(a.gt(b)),
            }),
            (FheValueInner::U32(a), FheValueInner::U32(b)) => Ok(Self {
                inner: FheValueInner::Bool(a.gt(b)),
            }),
            (FheValueInner::U64(a), FheValueInner::U64(b)) => Ok(Self {
                inner: FheValueInner::Bool(a.gt(b)),
            }),
            _ => Err(SdkError::Fhe(format!(
                "type mismatch in gt: {} vs {}",
                self.kind(),
                other.kind()
            ))),
        }
    }

    /// Compare greater-than-or-equal — returns a Bool-variant FheValue
    pub fn ge(&self, other: &Self) -> Result<Self> {
        use tfhe::prelude::FheOrd;
        match (&self.inner, &other.inner) {
            (FheValueInner::U8(a), FheValueInner::U8(b)) => Ok(Self {
                inner: FheValueInner::Bool(a.ge(b)),
            }),
            (FheValueInner::U16(a), FheValueInner::U16(b)) => Ok(Self {
                inner: FheValueInner::Bool(a.ge(b)),
            }),
            (FheValueInner::U32(a), FheValueInner::U32(b)) => Ok(Self {
                inner: FheValueInner::Bool(a.ge(b)),
            }),
            (FheValueInner::U64(a), FheValueInner::U64(b)) => Ok(Self {
                inner: FheValueInner::Bool(a.ge(b)),
            }),
            _ => Err(SdkError::Fhe(format!(
                "type mismatch in ge: {} vs {}",
                self.kind(),
                other.kind()
            ))),
        }
    }

    /// Boolean AND (only valid for Bool variants)
    pub fn and(&self, other: &Self) -> Result<Self> {
        match (&self.inner, &other.inner) {
            (FheValueInner::Bool(a), FheValueInner::Bool(b)) => Ok(Self {
                inner: FheValueInner::Bool(a & b),
            }),
            _ => Err(SdkError::Fhe(format!(
                "type mismatch in and: expected Bool, got {} and {}",
                self.kind(),
                other.kind()
            ))),
        }
    }

    /// Boolean OR (only valid for Bool variants)
    pub fn or(&self, other: &Self) -> Result<Self> {
        match (&self.inner, &other.inner) {
            (FheValueInner::Bool(a), FheValueInner::Bool(b)) => Ok(Self {
                inner: FheValueInner::Bool(a | b),
            }),
            _ => Err(SdkError::Fhe(format!(
                "type mismatch in or: expected Bool, got {} and {}",
                self.kind(),
                other.kind()
            ))),
        }
    }

    /// Boolean XOR (only valid for Bool variants)
    pub fn xor(&self, other: &Self) -> Result<Self> {
        match (&self.inner, &other.inner) {
            (FheValueInner::Bool(a), FheValueInner::Bool(b)) => Ok(Self {
                inner: FheValueInner::Bool(a ^ b),
            }),
            _ => Err(SdkError::Fhe(format!(
                "type mismatch in xor: expected Bool, got {} and {}",
                self.kind(),
                other.kind()
            ))),
        }
    }

    /// Boolean NOT (only valid for Bool variant)
    pub fn not(&self) -> Result<Self> {
        match &self.inner {
            FheValueInner::Bool(v) => Ok(Self {
                inner: FheValueInner::Bool(!v),
            }),
            _ => Err(SdkError::Fhe(format!(
                "type mismatch in not: expected Bool, got {}",
                self.kind()
            ))),
        }
    }
}

#[cfg(all(test, feature = "fhe"))]
mod tests {
    use super::*;
    use crate::fhe::FheKeys;

    #[test]
    fn test_fhe_value_arithmetic() {
        let keys = FheKeys::generate().expect("generate keys");
        keys.set_as_global_server_key();

        let a = FheValue::encrypt_u8(10, &keys).expect("encrypt a");
        let b = FheValue::encrypt_u8(5, &keys).expect("encrypt b");

        let sum = a.add(&b).expect("add");
        let result: u8 = sum.decrypt_u8(&keys).expect("decrypt sum");
        assert_eq!(result, 15);

        let diff = a.sub(&b).expect("sub");
        let result: u8 = diff.decrypt_u8(&keys).expect("decrypt diff");
        assert_eq!(result, 5);

        let product = a.mul(&b).expect("mul");
        let result: u8 = product.decrypt_u8(&keys).expect("decrypt product");
        assert_eq!(result, 50);
    }

    #[test]
    fn test_fhe_value_comparison() {
        let keys = FheKeys::generate().expect("generate keys");
        keys.set_as_global_server_key();

        let a = FheValue::encrypt_u8(10, &keys).expect("encrypt a");
        let b = FheValue::encrypt_u8(5, &keys).expect("encrypt b");
        let c = FheValue::encrypt_u8(10, &keys).expect("encrypt c");

        let gt = a.gt(&b).expect("gt");
        assert!(gt.decrypt_bool(&keys).expect("decrypt gt"));

        let lt = a.lt(&b).expect("lt");
        assert!(!lt.decrypt_bool(&keys).expect("decrypt lt"));

        let eq_result = a.eq(&c).expect("eq");
        assert!(eq_result.decrypt_bool(&keys).expect("decrypt eq"));

        let ne_result = a.ne(&b).expect("ne");
        assert!(ne_result.decrypt_bool(&keys).expect("decrypt ne"));

        let le = b.le(&a).expect("le");
        assert!(le.decrypt_bool(&keys).expect("decrypt le"));

        let ge = a.ge(&c).expect("ge");
        assert!(ge.decrypt_bool(&keys).expect("decrypt ge"));
    }

    #[test]
    fn test_fhe_value_boolean() {
        let keys = FheKeys::generate().expect("generate keys");
        keys.set_as_global_server_key();

        let t = FheValue::encrypt_bool(true, &keys).expect("encrypt true");
        let f = FheValue::encrypt_bool(false, &keys).expect("encrypt false");
        let t2 = FheValue::encrypt_bool(true, &keys).expect("encrypt true2");

        let and_result = t.and(&f).expect("and");
        assert!(!and_result.decrypt_bool(&keys).expect("decrypt and"));

        let or_result = t2.or(&f).expect("or");
        assert!(or_result.decrypt_bool(&keys).expect("decrypt or"));

        let xor_result = t.xor(&f).expect("xor");
        assert!(xor_result.decrypt_bool(&keys).expect("decrypt xor"));

        let not_t = t2.not().expect("not");
        assert!(!not_t.decrypt_bool(&keys).expect("decrypt not"));
    }

    #[test]
    fn test_fhe_value_type_mismatch() {
        let keys = FheKeys::generate().expect("generate keys");
        keys.set_as_global_server_key();

        let a = FheValue::encrypt_u8(10, &keys).expect("encrypt u8");
        let b = FheValue::encrypt_u16(5, &keys).expect("encrypt u16");

        assert!(a.add(&b).is_err(), "should fail on type mismatch");
        assert!(a.sub(&b).is_err(), "should fail on type mismatch");

        let bool_val = FheValue::encrypt_bool(true, &keys).expect("encrypt bool");
        assert!(bool_val.add(&a).is_err(), "bool + u8 should fail");
        assert!(a.and(&b).is_err(), "u8 and u16 should fail on and");
    }
}
