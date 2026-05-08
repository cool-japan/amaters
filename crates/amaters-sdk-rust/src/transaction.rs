//! Transaction support for AmateRS SDK.
//!
//! A `Transaction` buffers SET and DELETE operations locally and issues a
//! single atomic `execute_batch` RPC on `commit()`.  `rollback()` discards
//! the buffer without any network call.
//!
//! ## Cache note
//!
//! `AmateRSClient::execute_batch` does not invalidate the query cache (consistent
//! with the rest of `execute_batch` usage in this crate).  If the client has a
//! cache enabled, committing a transaction may leave stale entries for the keys
//! that were written.  Callers that require strict read-your-writes guarantees
//! should either disable the cache or call `client.cache().map(|c| c.invalidate(...))`
//! manually after commit.

use crate::client::AmateRSClient;
use crate::error::{Result, SdkError};
use amaters_core::{CipherBlob, Key, Query};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Internal state / operation types
// ---------------------------------------------------------------------------

/// Lifecycle state of a [`Transaction`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransactionState {
    Active,
    Committed,
    RolledBack,
}

/// A single buffered write operation.
#[derive(Debug, Clone)]
enum TransactionOp {
    Set {
        key: Key,
        value: CipherBlob,
    },
    Delete {
        key: Key,
    },
}

// ---------------------------------------------------------------------------
// Public Transaction type
// ---------------------------------------------------------------------------

/// A buffered, commit-or-rollback transaction over [`AmateRSClient`].
///
/// All writes are staged locally in a `Vec<TransactionOp>` until `commit()` is
/// called.  `commit()` issues a single `execute_batch` RPC so the writes are
/// applied atomically.  `rollback()` discards the local buffer with no network
/// call.
///
/// ## Reading inside a transaction
///
/// [`Transaction::get`] first inspects the local buffer using last-write-wins
/// semantics (reverse scan).  A buffered `Delete` for the queried key returns
/// `Ok(None)`.  If the key has not been written in this transaction the call
/// falls through to the server.
///
/// ## Drop behaviour
///
/// Dropping a transaction that is still `Active` and has un-committed
/// operations emits a `tracing::warn!` message.  The buffer is silently
/// discarded (no rollback RPC is issued — rollback is always local).
///
/// ## Construction
///
/// Prefer the factory method [`AmateRSClient::transaction`] over constructing
/// directly.
pub struct Transaction {
    collection: String,
    ops: Vec<TransactionOp>,
    client: Arc<AmateRSClient>,
    state: TransactionState,
}

impl Transaction {
    /// Create a new transaction bound to `collection`.
    ///
    /// Use [`AmateRSClient::transaction`] instead of calling this directly.
    pub fn new(client: Arc<AmateRSClient>, collection: impl Into<String>) -> Self {
        Self {
            collection: collection.into(),
            ops: Vec::new(),
            client,
            state: TransactionState::Active,
        }
    }

    // -----------------------------------------------------------------------
    // Write staging
    // -----------------------------------------------------------------------

    /// Stage a SET operation into the local buffer.
    ///
    /// The write is not applied to the server until [`Self::commit`] is called.
    ///
    /// # Errors
    ///
    /// Returns [`SdkError::InvalidState`] if the transaction is no longer
    /// active (already committed or rolled back).
    pub fn set(&mut self, key: Key, value: CipherBlob) -> Result<()> {
        self.ensure_active()?;
        self.ops.push(TransactionOp::Set { key, value });
        Ok(())
    }

    /// Stage a DELETE operation into the local buffer.
    ///
    /// The delete is not applied to the server until [`Self::commit`] is called.
    ///
    /// # Errors
    ///
    /// Returns [`SdkError::InvalidState`] if the transaction is no longer active.
    pub fn delete(&mut self, key: Key) -> Result<()> {
        self.ensure_active()?;
        self.ops.push(TransactionOp::Delete { key });
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Read (local buffer + server fallthrough)
    // -----------------------------------------------------------------------

    /// Read a key, consulting the local buffer first (last-write-wins), then
    /// the server.
    ///
    /// * A buffered `SET` returns the in-flight value without a server round-trip.
    /// * A buffered `DELETE` returns `Ok(None)` without a server round-trip.
    /// * If the key has not been touched in this transaction, the call falls
    ///   through to `client.get()`.
    ///
    /// # Errors
    ///
    /// Returns [`SdkError::InvalidState`] if the transaction is no longer active,
    /// or any error returned by the server fall-through.
    pub async fn get(&self, key: &Key) -> Result<Option<CipherBlob>> {
        self.ensure_active()?;

        // Walk ops in reverse for the most recent write to this key.
        for op in self.ops.iter().rev() {
            match op {
                TransactionOp::Set { key: k, value: v } if k == key => {
                    return Ok(Some(v.clone()));
                }
                TransactionOp::Delete { key: k } if k == key => {
                    return Ok(None);
                }
                _ => {}
            }
        }

        // Fall through to the server.
        self.client.get(&self.collection, key).await
    }

    // -----------------------------------------------------------------------
    // Commit / rollback
    // -----------------------------------------------------------------------

    /// Commit all buffered operations atomically via `execute_batch`.
    ///
    /// On success the transaction transitions to the `Committed` state.
    /// If the batch RPC fails, the state remains `Active` so the caller can
    /// retry or roll back.
    ///
    /// An empty transaction commits instantly without a network round-trip.
    ///
    /// # Errors
    ///
    /// * [`SdkError::InvalidState`] — already committed or rolled back.
    /// * Any `SdkError` returned by the underlying `execute_batch` RPC.
    pub async fn commit(&mut self) -> Result<()> {
        self.ensure_active()?;

        if !self.ops.is_empty() {
            let queries: Vec<Query> = self
                .ops
                .drain(..)
                .map(|op| match op {
                    TransactionOp::Set { key, value } => Query::Set {
                        collection: self.collection.clone(),
                        key,
                        value,
                    },
                    TransactionOp::Delete { key } => Query::Delete {
                        collection: self.collection.clone(),
                        key,
                    },
                })
                .collect();

            self.client.execute_batch(queries).await?;
        }

        self.state = TransactionState::Committed;
        Ok(())
    }

    /// Rollback by discarding the local buffer — no network call is made.
    ///
    /// # Errors
    ///
    /// Returns [`SdkError::InvalidState`] if the transaction is already
    /// committed or rolled back.
    pub fn rollback(&mut self) -> Result<()> {
        self.ensure_active()?;
        self.ops.clear();
        self.state = TransactionState::RolledBack;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Introspection
    // -----------------------------------------------------------------------

    /// Number of operations currently staged in the local buffer.
    pub fn pending_ops(&self) -> usize {
        self.ops.len()
    }

    /// Returns `true` if the transaction is still active (not yet committed or
    /// rolled back).
    pub fn is_active(&self) -> bool {
        self.state == TransactionState::Active
    }

    /// Returns the collection name this transaction is bound to.
    pub fn collection(&self) -> &str {
        &self.collection
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    fn ensure_active(&self) -> Result<()> {
        if self.state != TransactionState::Active {
            Err(SdkError::InvalidState(
                "transaction already committed or rolled back".to_string(),
            ))
        } else {
            Ok(())
        }
    }
}

impl Drop for Transaction {
    fn drop(&mut self) {
        if self.state == TransactionState::Active && !self.ops.is_empty() {
            tracing::warn!(
                pending_ops = self.ops.len(),
                collection = %self.collection,
                "Transaction dropped with uncommitted operation(s) — changes discarded",
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ClientConfig;
    use amaters_core::{CipherBlob, Key};

    /// Helper: build an offline client wrapped in Arc so Transaction::new can
    /// be called without a live server.
    fn offline_client() -> Arc<AmateRSClient> {
        let config = ClientConfig::new("http://127.0.0.1:50051");
        Arc::new(AmateRSClient::new_offline(config))
    }

    // -----------------------------------------------------------------------
    // State-machine tests (no server required)
    // -----------------------------------------------------------------------

    #[test]
    fn test_transaction_rollback_clears_buffer() {
        let client = offline_client();
        let mut tx = Transaction::new(client, "users");

        let key = Key::from_str("k1");
        let val = CipherBlob::new(vec![1, 2, 3]);
        tx.set(key, val).expect("set should succeed on active tx");
        assert_eq!(tx.pending_ops(), 1);

        tx.rollback().expect("rollback should succeed on active tx");
        assert_eq!(tx.pending_ops(), 0);
        assert!(!tx.is_active());
    }

    #[test]
    fn test_transaction_double_commit_returns_error() {
        // An empty transaction commits without a network call, so we can test
        // the state-machine offline.
        let client = offline_client();
        let mut tx = Transaction::new(client, "users");

        // First commit: no ops → fast path, no RPC.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build runtime");

        rt.block_on(async {
            tx.commit().await.expect("first commit should succeed");

            let err = tx
                .commit()
                .await
                .expect_err("second commit should return Err");
            assert!(
                matches!(err, SdkError::InvalidState(_)),
                "expected InvalidState, got: {err}"
            );
        });
    }

    #[test]
    fn test_transaction_commit_then_rollback_is_error() {
        let client = offline_client();
        let mut tx = Transaction::new(client, "users");

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build runtime");

        rt.block_on(async {
            tx.commit().await.expect("commit should succeed");

            let err = tx
                .rollback()
                .expect_err("rollback after commit should return Err");
            assert!(
                matches!(err, SdkError::InvalidState(_)),
                "expected InvalidState, got: {err}"
            );
        });
    }

    #[test]
    fn test_transaction_rollback_then_commit_is_error() {
        let client = offline_client();
        let mut tx = Transaction::new(client, "users");

        tx.rollback().expect("rollback should succeed on active tx");

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build runtime");

        rt.block_on(async {
            let err = tx
                .commit()
                .await
                .expect_err("commit after rollback should return Err");
            assert!(
                matches!(err, SdkError::InvalidState(_)),
                "expected InvalidState, got: {err}"
            );
        });
    }

    // -----------------------------------------------------------------------
    // Local-buffer read tests (no server required)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_transaction_read_sees_local_set() {
        let client = offline_client();
        let mut tx = Transaction::new(client, "users");

        let key = Key::from_str("local_key");
        let val = CipherBlob::new(vec![10, 20, 30]);
        tx.set(key.clone(), val.clone())
            .expect("set should succeed");

        let result = tx
            .get(&key)
            .await
            .expect("get should succeed (local hit)");
        assert_eq!(
            result.as_ref().map(|b| b.to_vec()),
            Some(val.to_vec()),
            "get should return the locally staged value"
        );
    }

    #[tokio::test]
    async fn test_transaction_read_sees_local_delete_as_none() {
        let client = offline_client();
        let mut tx = Transaction::new(client, "users");

        let key = Key::from_str("will_delete");
        let val = CipherBlob::new(vec![1]);
        tx.set(key.clone(), val).expect("set should succeed");
        tx.delete(key.clone()).expect("delete should succeed");

        let result = tx
            .get(&key)
            .await
            .expect("get should succeed (local delete hit)");
        assert!(
            result.is_none(),
            "locally deleted key should appear as None"
        );
    }

    #[tokio::test]
    async fn test_transaction_read_last_write_wins() {
        let client = offline_client();
        let mut tx = Transaction::new(client, "users");

        let key = Key::from_str("overwritten");
        let v1 = CipherBlob::new(vec![1]);
        let v2 = CipherBlob::new(vec![2]);
        tx.set(key.clone(), v1).expect("first set");
        tx.set(key.clone(), v2.clone()).expect("second set");

        let result = tx
            .get(&key)
            .await
            .expect("get should succeed");
        assert_eq!(
            result.as_ref().map(|b| b.to_vec()),
            Some(v2.to_vec()),
            "last write should win"
        );
    }

    // -----------------------------------------------------------------------
    // Drop / tracing tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_transaction_empty_drop_no_warn() {
        // An empty transaction dropped while Active should NOT emit a warning.
        // We cannot assert "no log was emitted" without tracing-test, but we
        // can at least verify the Drop impl path is exercised without panic.
        let client = offline_client();
        let tx = Transaction::new(client, "noop");
        drop(tx); // should not panic or warn
    }

    #[tracing_test::traced_test]
    #[test]
    fn test_transaction_drop_warns_uncommitted() {
        let client = offline_client();
        let mut tx = Transaction::new(client, "events");

        let key = Key::from_str("pending");
        let val = CipherBlob::new(vec![0xFF]);
        tx.set(key, val).expect("set should succeed");

        // Drop without commit/rollback — should trigger the warn! in Drop.
        drop(tx);

        assert!(
            logs_contain("Transaction dropped with uncommitted operation(s)"),
            "expected a tracing warn about uncommitted ops"
        );
    }
}
