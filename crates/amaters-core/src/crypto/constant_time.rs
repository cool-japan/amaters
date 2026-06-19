//! Constant-time comparison and selection primitives.

/// Constant-time equality comparison of two byte slices.
///
/// # Example
///
/// ```
/// use amaters_core::crypto::constant_time::constant_time_eq;
///
/// let a = b"secret_token_abc";
/// let b_eq = b"secret_token_abc";
/// let c = b"different_token!";
///
/// assert!(constant_time_eq(a, b_eq));
/// assert!(!constant_time_eq(a, c));
/// ```
#[inline]
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Constant-time byte selection.
///
/// Returns `a` if `choice` is non-zero; returns `b` if `choice == 0`.
#[inline]
pub fn constant_time_select(choice: u8, a: u8, b: u8) -> u8 {
    // Normalise to exactly 0 or 1 so wrapping_neg produces a full 0x00/0xFF mask.
    let bit = choice.min(1);
    let mask = (bit as i8).wrapping_neg() as u8;
    (mask & a) | (!mask & b)
}

/// Constant-time slice selection.
///
/// Writes `a` into `out` if `choice` is non-zero; writes `b` into `out` if `choice == 0`.
pub fn constant_time_select_slice(choice: u8, a: &[u8], b: &[u8], out: &mut [u8]) {
    assert_eq!(a.len(), b.len(), "a and b must have equal length");
    assert_eq!(a.len(), out.len(), "out must have same length as a and b");
    // Normalise to exactly 0 or 1 so wrapping_neg produces a full 0x00/0xFF mask.
    let bit = choice.min(1);
    let mask = (bit as i8).wrapping_neg() as u8;
    for ((ai, bi), oi) in a.iter().zip(b.iter()).zip(out.iter_mut()) {
        *oi = (mask & ai) | (!mask & bi);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn test_constant_time_eq_identical() {
        assert!(constant_time_eq(b"", b""));
        assert!(constant_time_eq(b"hello world", b"hello world"));
        let v: Vec<u8> = (0u8..=255).collect();
        assert!(constant_time_eq(&v, &v.clone()));
    }

    #[test]
    fn test_constant_time_eq_different_content() {
        assert!(!constant_time_eq(b"hello", b"world"));
        assert!(!constant_time_eq(b"aaa", b"aab"));
        let mut a = vec![0u8; 64];
        let mut b = vec![0u8; 64];
        b[0] = 1;
        assert!(!constant_time_eq(&a, &b));
        a[63] = 1;
        b[0] = 0;
        assert!(!constant_time_eq(&a, &b));
    }

    #[test]
    fn test_constant_time_eq_different_length() {
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(!constant_time_eq(b"", b"a"));
        assert!(!constant_time_eq(b"a", b""));
    }

    #[test]
    fn test_constant_time_select_byte() {
        assert_eq!(constant_time_select(1, 0xAA, 0xBB), 0xAA);
        assert_eq!(constant_time_select(0, 0xAA, 0xBB), 0xBB);
        assert_eq!(constant_time_select(2, 0xAA, 0xBB), 0xAA);
    }

    #[test]
    fn test_constant_time_select_slice() {
        let a = [0xAAu8; 8];
        let b = [0xBBu8; 8];
        let mut out = [0u8; 8];
        constant_time_select_slice(1, &a, &b, &mut out);
        assert_eq!(out, a);
        constant_time_select_slice(0, &a, &b, &mut out);
        assert_eq!(out, b);
    }

    #[test]
    fn test_constant_time_eq_timing_ratio() {
        let a = vec![0u8; 1024];
        let b_eq = vec![0u8; 1024];
        let mut b_diff = vec![0u8; 1024];
        b_diff[0] = 1;

        let n = 10_000usize;

        let t_eq = {
            let start = Instant::now();
            for _ in 0..n {
                let _ = constant_time_eq(&a, &b_eq);
            }
            start.elapsed()
        };

        let t_diff = {
            let start = Instant::now();
            for _ in 0..n {
                let _ = constant_time_eq(&a, &b_diff);
            }
            start.elapsed()
        };

        let ratio = t_diff.as_nanos() as f64 / t_eq.as_nanos().max(1) as f64;
        assert!(
            ratio > 0.2 && ratio < 5.0,
            "timing ratio {ratio:.2} suggests non-constant-time behavior (t_eq={t_eq:?}, t_diff={t_diff:?})"
        );
    }
}
