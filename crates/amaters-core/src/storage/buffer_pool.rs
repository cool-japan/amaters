//! Buffer pool for reusable byte buffers organized by size class.
//!
//! The buffer pool eliminates frequent heap allocations by maintaining free lists
//! of pre-allocated buffers. When a [`PooledBuffer`] is dropped it is automatically
//! returned to the pool for future reuse.
//!
//! # Size classes
//!
//! Buffers are grouped into nine size classes (in bytes):
//! 4 KB, 8 KB, 16 KB, 32 KB, 64 KB, 128 KB, 256 KB, 512 KB, 1 MB.

use parking_lot::Mutex;
use std::ops::{Deref, DerefMut};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

/// The nine supported size classes (bytes).
const SIZE_CLASSES: [usize; 9] = [
    4_096,     // 4 KB
    8_192,     // 8 KB
    16_384,    // 16 KB
    32_768,    // 32 KB
    65_536,    // 64 KB
    131_072,   // 128 KB
    262_144,   // 256 KB
    524_288,   // 512 KB
    1_048_576, // 1 MB
];

// ---------------------------------------------------------------------------
// BufferPoolStats
// ---------------------------------------------------------------------------

/// Snapshot of [`BufferPool`] counters.
#[derive(Debug, Clone)]
pub struct BufferPoolStats {
    /// Total number of [`PooledBuffer`] acquisitions.
    pub allocations: u64,
    /// Number of acquisitions satisfied from the free list.
    pub recycles: u64,
    /// Number of acquisitions that required a fresh heap allocation (bypassed
    /// the pool, either because the free list was empty or the requested size
    /// exceeded the largest size class).
    pub misses: u64,
    /// `recycles / allocations`, or `0.0` when `allocations == 0`.
    pub recycle_rate: f64,
}

// ---------------------------------------------------------------------------
// BufferPool
// ---------------------------------------------------------------------------

/// A pool of reusable byte buffers organized by size class.
///
/// Obtain a buffer with [`BufferPool::acquire`]; it is automatically returned
/// when the [`PooledBuffer`] is dropped.
pub struct BufferPool {
    /// One free list per size class.
    free_lists: Arc<[Mutex<Vec<Vec<u8>>>; 9]>,
    /// Maximum number of buffers to retain per size class.
    capacity_per_class: usize,
    /// Total [`acquire`] calls.
    allocations: AtomicU64,
    /// Acquisitions served from the free list.
    recycles: AtomicU64,
    /// Acquisitions that fell back to fresh allocation.
    misses: AtomicU64,
}

impl BufferPool {
    /// Create a new pool wrapping at most `capacity_per_class` idle buffers per
    /// size class.
    pub fn new(capacity_per_class: usize) -> Arc<Self> {
        Arc::new(Self {
            free_lists: Arc::new([
                Mutex::new(Vec::new()),
                Mutex::new(Vec::new()),
                Mutex::new(Vec::new()),
                Mutex::new(Vec::new()),
                Mutex::new(Vec::new()),
                Mutex::new(Vec::new()),
                Mutex::new(Vec::new()),
                Mutex::new(Vec::new()),
                Mutex::new(Vec::new()),
            ]),
            capacity_per_class,
            allocations: AtomicU64::new(0),
            recycles: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        })
    }

    /// Acquire a buffer large enough to hold `min_size` bytes.
    ///
    /// The returned [`PooledBuffer`] tracks its size class and will return
    /// itself to the pool when dropped.
    ///
    /// If `min_size` exceeds the largest size class (1 MB), a fresh allocation
    /// is issued on every call and the buffer is **not** pooled on release.  In
    /// this case `misses` is incremented.
    pub fn acquire(self: &Arc<Self>, min_size: usize) -> PooledBuffer {
        self.allocations.fetch_add(1, Ordering::Relaxed);

        // Find the smallest size class that fits.
        let class_idx = SIZE_CLASSES.iter().position(|&cap| cap >= min_size);

        match class_idx {
            Some(idx) => {
                let class_size = SIZE_CLASSES[idx];
                // Try to pop an existing buffer from the free list.
                let maybe_buf = self.free_lists[idx].lock().pop();
                match maybe_buf {
                    Some(mut buf) => {
                        // Reuse: clear content, keep allocation.
                        buf.clear();
                        buf.resize(class_size, 0u8);
                        self.recycles.fetch_add(1, Ordering::Relaxed);
                        PooledBuffer {
                            data: Some(buf),
                            pool: Arc::clone(self),
                            size_class: idx,
                        }
                    }
                    None => {
                        // Free list is empty — allocate fresh.
                        self.misses.fetch_add(1, Ordering::Relaxed);
                        let buf = vec![0u8; class_size];
                        PooledBuffer {
                            data: Some(buf),
                            pool: Arc::clone(self),
                            size_class: idx,
                        }
                    }
                }
            }
            None => {
                // Requested size exceeds every size class — allocate without
                // pooling (size_class == usize::MAX signals "no pool slot").
                self.misses.fetch_add(1, Ordering::Relaxed);
                let buf = vec![0u8; min_size];
                PooledBuffer {
                    data: Some(buf),
                    pool: Arc::clone(self),
                    size_class: usize::MAX,
                }
            }
        }
    }

    /// Return a buffer to the pool.
    ///
    /// This is called automatically by [`PooledBuffer::drop`].
    pub fn release(&self, buffer: Vec<u8>, size_class: usize) {
        // Oversized buffers (sentinel class) are simply dropped.
        if size_class >= SIZE_CLASSES.len() {
            return;
        }
        let mut list = self.free_lists[size_class].lock();
        if list.len() < self.capacity_per_class {
            list.push(buffer);
        }
        // If the list is full the buffer is simply dropped (list lock released).
    }

    /// Total number of `acquire` calls.
    pub fn allocations(&self) -> u64 {
        self.allocations.load(Ordering::Relaxed)
    }

    /// Number of acquisitions served from the free list.
    pub fn recycles(&self) -> u64 {
        self.recycles.load(Ordering::Relaxed)
    }

    /// Number of acquisitions that required a fresh heap allocation.
    pub fn misses(&self) -> u64 {
        self.misses.load(Ordering::Relaxed)
    }

    /// Return a snapshot of all counters.
    pub fn stats(&self) -> BufferPoolStats {
        let allocations = self.allocations();
        let recycles = self.recycles();
        let misses = self.misses();
        let recycle_rate = if allocations == 0 {
            0.0
        } else {
            recycles as f64 / allocations as f64
        };
        BufferPoolStats {
            allocations,
            recycles,
            misses,
            recycle_rate,
        }
    }
}

// ---------------------------------------------------------------------------
// PooledBuffer
// ---------------------------------------------------------------------------

/// A byte buffer borrowed from a [`BufferPool`].
///
/// The buffer is automatically returned to the pool when this value is dropped.
pub struct PooledBuffer {
    /// `None` only transiently during `Drop`.
    data: Option<Vec<u8>>,
    pool: Arc<BufferPool>,
    /// Index into [`SIZE_CLASSES`], or `usize::MAX` for oversized allocations.
    size_class: usize,
}

impl Drop for PooledBuffer {
    fn drop(&mut self) {
        let buf = self.data.take().unwrap_or_default();
        self.pool.release(buf, self.size_class);
    }
}

impl Deref for PooledBuffer {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        self.data.as_deref().unwrap_or(&[])
    }
}

impl DerefMut for PooledBuffer {
    fn deref_mut(&mut self) -> &mut [u8] {
        self.data.as_deref_mut().unwrap_or(&mut [])
    }
}

impl AsRef<[u8]> for PooledBuffer {
    fn as_ref(&self) -> &[u8] {
        self.deref()
    }
}

impl AsMut<[u8]> for PooledBuffer {
    fn as_mut(&mut self) -> &mut [u8] {
        self.deref_mut()
    }
}

impl PooledBuffer {
    /// Length of the buffer in bytes.
    pub fn len(&self) -> usize {
        self.data.as_ref().map_or(0, |d| d.len())
    }

    /// `true` if the buffer has zero length.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Allocated capacity of the inner `Vec`.
    pub fn capacity(&self) -> usize {
        self.data.as_ref().map_or(0, |d| d.capacity())
    }

    /// Immutable byte slice view.
    pub fn as_slice(&self) -> &[u8] {
        self.deref()
    }

    /// Mutable byte slice view.
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        self.deref_mut()
    }

    /// Resize the inner `Vec`, filling new elements with `value`.
    pub fn resize(&mut self, new_len: usize, value: u8) {
        if let Some(ref mut v) = self.data {
            v.resize(new_len, value);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that a buffer acquired for `min_size` bytes has at least that
    /// many bytes available.
    #[test]
    fn test_buffer_pool_basic_acquire() {
        let pool = BufferPool::new(4);
        let buf = pool.acquire(1_000);
        assert!(
            buf.len() >= 1_000,
            "buffer must be at least the requested size"
        );
    }

    /// Drop a buffer, re-acquire one, and verify the recycle counter climbs.
    #[test]
    fn test_buffer_pool_recycle() {
        let pool = BufferPool::new(4);
        let buf = pool.acquire(4_096);
        drop(buf);
        let _buf2 = pool.acquire(4_096);
        assert!(pool.recycles() >= 1, "second acquire should be a recycle");
    }

    /// Verify that buffers are rounded up to the correct size-class capacity.
    ///
    /// Requested sizes and expected capacities:
    /// - 1 byte  → 4 KB class  (4 096)
    /// - 4 096   → 4 KB class  (4 096)
    /// - 8 193   → 16 KB class (16 384)
    /// - 65 537  → 128 KB class (131 072)
    #[test]
    fn test_buffer_pool_size_classes() {
        let pool = BufferPool::new(4);

        let cases: &[(usize, usize)] = &[
            (1, SIZE_CLASSES[0]),          // 1 → 4 KB
            (4_096, SIZE_CLASSES[0]),      // exactly 4 KB → 4 KB
            (8_193, SIZE_CLASSES[2]),      // just over 8 KB → 16 KB
            (65_537, SIZE_CLASSES[4 + 1]), // just over 64 KB → 128 KB
        ];

        for &(req, expected_cap) in cases {
            let buf = pool.acquire(req);
            assert_eq!(
                buf.capacity(),
                expected_cap,
                "request {} → expected size-class capacity {}",
                req,
                expected_cap
            );
        }
    }

    /// The free list must not grow beyond `capacity_per_class`.
    #[test]
    fn test_buffer_pool_capacity_limit() {
        let capacity = 3usize;
        let pool = BufferPool::new(capacity);

        // Acquire capacity + 1 buffers, all in the same size class.
        let buffers: Vec<_> = (0..capacity + 1).map(|_| pool.acquire(4_096)).collect();
        // Drop them all — the pool may keep at most `capacity`.
        drop(buffers);

        let free_count = pool.free_lists[0].lock().len();
        assert!(
            free_count <= capacity,
            "free list must not exceed capacity (got {})",
            free_count
        );
    }

    /// Write via `DerefMut`, read via `Deref`.
    #[test]
    fn test_pooled_buffer_deref() {
        let pool = BufferPool::new(4);
        let mut buf = pool.acquire(4_096);

        buf[0] = 0xAA;
        buf[1] = 0xBB;

        assert_eq!(buf[0], 0xAA);
        assert_eq!(buf[1], 0xBB);
    }

    /// Counters must reflect acquire / recycle / miss semantics.
    #[test]
    fn test_buffer_pool_stats() {
        let pool = BufferPool::new(4);

        // First acquire: miss (free list is empty).
        let b1 = pool.acquire(4_096);
        assert_eq!(pool.allocations(), 1);
        assert_eq!(pool.misses(), 1);
        assert_eq!(pool.recycles(), 0);

        // Release b1 back to the pool.
        drop(b1);

        // Second acquire: recycle.
        let _b2 = pool.acquire(4_096);
        assert_eq!(pool.allocations(), 2);
        assert_eq!(pool.misses(), 1);
        assert_eq!(pool.recycles(), 1);

        let stats = pool.stats();
        assert_eq!(stats.allocations, 2);
        assert_eq!(stats.recycles, 1);
        assert!((stats.recycle_rate - 0.5).abs() < f64::EPSILON);
    }
}
