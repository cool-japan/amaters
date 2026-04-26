//! AmateRS Query Language (AQL) types
//!
//! Defines the Abstract Syntax Tree (AST) for queries on encrypted data.

use super::{CipherBlob, Key};

/// Maximum byte length considered "plaintext" for predicate evaluation.
///
/// Real FHE ciphertexts are kilobytes in size. Values that fit within this
/// threshold are treated as raw (non-encrypted) byte sequences, enabling
/// server-side predicate evaluation without FHE machinery.
const PLAINTEXT_MAX_BYTES: usize = 64;

/// Top-level query type
#[derive(Debug, Clone, PartialEq)]
pub enum Query {
    /// Get a single value by key
    Get { collection: String, key: Key },
    /// Set a value
    Set {
        collection: String,
        key: Key,
        value: CipherBlob,
    },
    /// Delete a key
    Delete { collection: String, key: Key },
    /// Filter collection by predicate
    Filter {
        collection: String,
        predicate: Predicate,
    },
    /// Update values matching predicate
    Update {
        collection: String,
        predicate: Predicate,
        updates: Vec<Update>,
    },
    /// Range scan
    Range {
        collection: String,
        start: Key,
        end: Key,
    },
}

/// Predicate for filtering (executed on encrypted data via FHE)
#[derive(Debug, Clone, PartialEq)]
pub enum Predicate {
    /// Equality test
    Eq(ColumnRef, CipherBlob),
    /// Greater than
    Gt(ColumnRef, CipherBlob),
    /// Less than
    Lt(ColumnRef, CipherBlob),
    /// Greater than or equal
    Gte(ColumnRef, CipherBlob),
    /// Less than or equal
    Lte(ColumnRef, CipherBlob),
    /// Logical AND
    And(Box<Predicate>, Box<Predicate>),
    /// Logical OR
    Or(Box<Predicate>, Box<Predicate>),
    /// Logical NOT
    Not(Box<Predicate>),
}

impl Predicate {
    /// Evaluate this predicate against a stored plaintext value.
    ///
    /// Returns `Some(true)` if the value matches, `Some(false)` if it does not,
    /// or `None` if plaintext evaluation is not applicable (e.g. either the
    /// stored value or the comparison value exceeds `PLAINTEXT_MAX_BYTES` (64 bytes),
    /// indicating an FHE ciphertext rather than raw bytes).
    ///
    /// Values are compared lexicographically as big-endian byte sequences.
    /// For single-byte unsigned integers this is equivalent to numeric ordering.
    pub fn evaluate_plaintext(&self, stored: &CipherBlob) -> Option<bool> {
        match self {
            Predicate::Eq(_, rhs) => {
                if stored.as_bytes().len() > PLAINTEXT_MAX_BYTES
                    || rhs.as_bytes().len() > PLAINTEXT_MAX_BYTES
                {
                    return None;
                }
                Some(stored.as_bytes() == rhs.as_bytes())
            }
            Predicate::Gt(_, rhs) => {
                if stored.as_bytes().len() > PLAINTEXT_MAX_BYTES
                    || rhs.as_bytes().len() > PLAINTEXT_MAX_BYTES
                {
                    return None;
                }
                Some(stored.as_bytes() > rhs.as_bytes())
            }
            Predicate::Lt(_, rhs) => {
                if stored.as_bytes().len() > PLAINTEXT_MAX_BYTES
                    || rhs.as_bytes().len() > PLAINTEXT_MAX_BYTES
                {
                    return None;
                }
                Some(stored.as_bytes() < rhs.as_bytes())
            }
            Predicate::Gte(_, rhs) => {
                if stored.as_bytes().len() > PLAINTEXT_MAX_BYTES
                    || rhs.as_bytes().len() > PLAINTEXT_MAX_BYTES
                {
                    return None;
                }
                Some(stored.as_bytes() >= rhs.as_bytes())
            }
            Predicate::Lte(_, rhs) => {
                if stored.as_bytes().len() > PLAINTEXT_MAX_BYTES
                    || rhs.as_bytes().len() > PLAINTEXT_MAX_BYTES
                {
                    return None;
                }
                Some(stored.as_bytes() <= rhs.as_bytes())
            }
            Predicate::And(left, right) => {
                let l = left.evaluate_plaintext(stored)?;
                if !l {
                    return Some(false);
                }
                right.evaluate_plaintext(stored)
            }
            Predicate::Or(left, right) => {
                let l = left.evaluate_plaintext(stored)?;
                if l {
                    return Some(true);
                }
                right.evaluate_plaintext(stored)
            }
            Predicate::Not(inner) => {
                let v = inner.evaluate_plaintext(stored)?;
                Some(!v)
            }
        }
    }
}

/// Reference to a column in the encrypted data
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ColumnRef {
    pub name: String,
}

impl ColumnRef {
    /// Create a new column reference
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

/// Update operation
#[derive(Debug, Clone, PartialEq)]
pub enum Update {
    /// Set column to value
    Set(ColumnRef, CipherBlob),
    /// Add to column (FHE addition)
    Add(ColumnRef, CipherBlob),
    /// Multiply column (FHE multiplication)
    Mul(ColumnRef, CipherBlob),
}

/// Query builder for fluent API
pub struct QueryBuilder {
    collection: String,
}

impl QueryBuilder {
    /// Create a new query builder for a collection
    pub fn new(collection: impl Into<String>) -> Self {
        Self {
            collection: collection.into(),
        }
    }

    /// Build a Get query
    pub fn get(self, key: Key) -> Query {
        Query::Get {
            collection: self.collection,
            key,
        }
    }

    /// Build a Set query
    pub fn set(self, key: Key, value: CipherBlob) -> Query {
        Query::Set {
            collection: self.collection,
            key,
            value,
        }
    }

    /// Build a Delete query
    pub fn delete(self, key: Key) -> Query {
        Query::Delete {
            collection: self.collection,
            key,
        }
    }

    /// Build a Filter query
    pub fn filter(self, predicate: Predicate) -> Query {
        Query::Filter {
            collection: self.collection,
            predicate,
        }
    }

    /// Build an Update query
    pub fn update(self, predicate: Predicate, updates: Vec<Update>) -> Query {
        Query::Update {
            collection: self.collection,
            predicate,
            updates,
        }
    }

    /// Build a Range query
    pub fn range(self, start: Key, end: Key) -> Query {
        Query::Range {
            collection: self.collection,
            start,
            end,
        }
    }
}

/// Helper function to create a column reference
pub fn col(name: impl Into<String>) -> ColumnRef {
    ColumnRef::new(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_builder() {
        let query = QueryBuilder::new("users").get(Key::from_str("user:123"));

        match query {
            Query::Get { collection, key } => {
                assert_eq!(collection, "users");
                assert_eq!(key, Key::from_str("user:123"));
            }
            _ => panic!("Expected Get query"),
        }
    }

    #[test]
    fn test_predicate_and() {
        let pred1 = Predicate::Eq(col("age"), CipherBlob::new(vec![1, 2, 3]));
        let pred2 = Predicate::Gt(col("salary"), CipherBlob::new(vec![4, 5, 6]));
        let pred = Predicate::And(Box::new(pred1), Box::new(pred2));

        assert!(matches!(pred, Predicate::And(_, _)));
    }

    #[test]
    fn test_update_operations() {
        let update1 = Update::Set(col("status"), CipherBlob::new(vec![1]));
        let update2 = Update::Add(col("counter"), CipherBlob::new(vec![2]));

        assert!(matches!(update1, Update::Set(_, _)));
        assert!(matches!(update2, Update::Add(_, _)));
    }

    #[test]
    fn test_column_ref() {
        let col1 = col("name");
        let col2 = col("name");

        assert_eq!(col1, col2);
    }
}
