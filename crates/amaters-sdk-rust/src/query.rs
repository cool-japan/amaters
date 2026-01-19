//! Query builder for fluent API construction

use amaters_core::{CipherBlob, ColumnRef, Key, Predicate, Query, Update};

/// Query builder with fluent API
pub struct FluentQueryBuilder {
    collection: String,
}

impl FluentQueryBuilder {
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

    /// Start building a filter with predicate builder
    pub fn where_clause(self) -> PredicateBuilder {
        PredicateBuilder::new(self.collection)
    }
}

/// Predicate builder for constructing complex predicates
pub struct PredicateBuilder {
    collection: String,
}

impl PredicateBuilder {
    /// Create a new predicate builder
    pub fn new(collection: impl Into<String>) -> Self {
        Self {
            collection: collection.into(),
        }
    }

    /// Create an equality predicate
    pub fn eq(self, column: ColumnRef, value: CipherBlob) -> FilterBuilder {
        FilterBuilder::new(self.collection, Predicate::Eq(column, value))
    }

    /// Create a greater than predicate
    pub fn gt(self, column: ColumnRef, value: CipherBlob) -> FilterBuilder {
        FilterBuilder::new(self.collection, Predicate::Gt(column, value))
    }

    /// Create a less than predicate
    pub fn lt(self, column: ColumnRef, value: CipherBlob) -> FilterBuilder {
        FilterBuilder::new(self.collection, Predicate::Lt(column, value))
    }

    /// Create a greater than or equal predicate
    pub fn gte(self, column: ColumnRef, value: CipherBlob) -> FilterBuilder {
        FilterBuilder::new(self.collection, Predicate::Gte(column, value))
    }

    /// Create a less than or equal predicate
    pub fn lte(self, column: ColumnRef, value: CipherBlob) -> FilterBuilder {
        FilterBuilder::new(self.collection, Predicate::Lte(column, value))
    }
}

/// Filter builder for combining predicates
pub struct FilterBuilder {
    collection: String,
    predicate: Predicate,
}

impl FilterBuilder {
    /// Create a new filter builder
    fn new(collection: String, predicate: Predicate) -> Self {
        Self {
            collection,
            predicate,
        }
    }

    /// Add an AND condition
    pub fn and(mut self, other: Predicate) -> Self {
        self.predicate = Predicate::And(Box::new(self.predicate), Box::new(other));
        self
    }

    /// Add an OR condition
    pub fn or(mut self, other: Predicate) -> Self {
        self.predicate = Predicate::Or(Box::new(self.predicate), Box::new(other));
        self
    }

    /// Add a NOT wrapper
    #[allow(clippy::should_implement_trait)]
    pub fn not(mut self) -> Self {
        self.predicate = Predicate::Not(Box::new(self.predicate));
        self
    }

    /// Build the filter query
    pub fn build(self) -> Query {
        Query::Filter {
            collection: self.collection,
            predicate: self.predicate,
        }
    }

    /// Build an update query with this predicate
    pub fn update(self, updates: Vec<Update>) -> Query {
        Query::Update {
            collection: self.collection,
            predicate: self.predicate,
            updates,
        }
    }
}

/// Helper function to create a query builder
pub fn query(collection: impl Into<String>) -> FluentQueryBuilder {
    FluentQueryBuilder::new(collection)
}

#[cfg(test)]
mod tests {
    use super::*;
    use amaters_core::col;

    #[test]
    fn test_query_builder() {
        let q = query("users").get(Key::from_str("user:1"));
        match q {
            Query::Get { collection, key } => {
                assert_eq!(collection, "users");
                assert_eq!(key.to_string_lossy(), "user:1");
            }
            _ => panic!("expected Get query"),
        }
    }

    #[test]
    fn test_filter_builder() {
        let q = query("users")
            .where_clause()
            .eq(col("age"), CipherBlob::new(vec![1, 2, 3]))
            .build();

        match q {
            Query::Filter {
                collection,
                predicate,
            } => {
                assert_eq!(collection, "users");
                assert!(matches!(predicate, Predicate::Eq(_, _)));
            }
            _ => panic!("expected Filter query"),
        }
    }

    #[test]
    fn test_complex_filter() {
        let q = query("users")
            .where_clause()
            .eq(col("status"), CipherBlob::new(vec![1]))
            .and(Predicate::Gt(col("age"), CipherBlob::new(vec![18])))
            .build();

        match q {
            Query::Filter {
                collection,
                predicate,
            } => {
                assert_eq!(collection, "users");
                assert!(matches!(predicate, Predicate::And(_, _)));
            }
            _ => panic!("expected Filter query"),
        }
    }

    #[test]
    fn test_update_builder() {
        let updates = vec![Update::Set(col("status"), CipherBlob::new(vec![2]))];

        let q = query("users")
            .where_clause()
            .eq(col("id"), CipherBlob::new(vec![1]))
            .update(updates);

        match q {
            Query::Update {
                collection,
                predicate,
                updates,
            } => {
                assert_eq!(collection, "users");
                assert!(matches!(predicate, Predicate::Eq(_, _)));
                assert_eq!(updates.len(), 1);
            }
            _ => panic!("expected Update query"),
        }
    }

    #[test]
    fn test_range_query() {
        let q = query("data").range(Key::from_str("a"), Key::from_str("z"));

        match q {
            Query::Range {
                collection,
                start,
                end,
            } => {
                assert_eq!(collection, "data");
                assert_eq!(start.to_string_lossy(), "a");
                assert_eq!(end.to_string_lossy(), "z");
            }
            _ => panic!("expected Range query"),
        }
    }
}
