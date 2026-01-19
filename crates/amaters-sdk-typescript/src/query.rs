//! Query builder for TypeScript SDK
//!
//! This module provides a fluent query builder API for constructing
//! database queries in TypeScript/JavaScript.

use crate::types::{CipherBlob, ColumnRef, Key};
use wasm_bindgen::prelude::*;

/// Query type enumeration
#[wasm_bindgen]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryType {
    /// Get a single value by key
    Get,
    /// Set a value
    Set,
    /// Delete a key
    Delete,
    /// Filter by predicate
    Filter,
    /// Update by predicate
    Update,
    /// Range query
    Range,
}

/// Predicate type for filtering
#[wasm_bindgen]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredicateType {
    /// Equality
    Eq,
    /// Greater than
    Gt,
    /// Less than
    Lt,
    /// Greater than or equal
    Gte,
    /// Less than or equal
    Lte,
    /// Logical AND
    And,
    /// Logical OR
    Or,
    /// Logical NOT
    Not,
}

/// Update operation type
#[wasm_bindgen]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateType {
    /// Set column to value
    Set,
    /// Add to column (FHE addition)
    Add,
    /// Multiply column (FHE multiplication)
    Mul,
}

/// Predicate for filtering queries
#[wasm_bindgen]
#[derive(Debug, Clone)]
pub struct Predicate {
    predicate_type: PredicateType,
    column: Option<ColumnRef>,
    value: Option<CipherBlob>,
    left: Option<Box<Predicate>>,
    right: Option<Box<Predicate>>,
}

#[wasm_bindgen]
impl Predicate {
    /// Create an equality predicate
    #[wasm_bindgen]
    pub fn eq(column: ColumnRef, value: CipherBlob) -> Predicate {
        Predicate {
            predicate_type: PredicateType::Eq,
            column: Some(column),
            value: Some(value),
            left: None,
            right: None,
        }
    }

    /// Create a greater than predicate
    #[wasm_bindgen]
    pub fn gt(column: ColumnRef, value: CipherBlob) -> Predicate {
        Predicate {
            predicate_type: PredicateType::Gt,
            column: Some(column),
            value: Some(value),
            left: None,
            right: None,
        }
    }

    /// Create a less than predicate
    #[wasm_bindgen]
    pub fn lt(column: ColumnRef, value: CipherBlob) -> Predicate {
        Predicate {
            predicate_type: PredicateType::Lt,
            column: Some(column),
            value: Some(value),
            left: None,
            right: None,
        }
    }

    /// Create a greater than or equal predicate
    #[wasm_bindgen]
    pub fn gte(column: ColumnRef, value: CipherBlob) -> Predicate {
        Predicate {
            predicate_type: PredicateType::Gte,
            column: Some(column),
            value: Some(value),
            left: None,
            right: None,
        }
    }

    /// Create a less than or equal predicate
    #[wasm_bindgen]
    pub fn lte(column: ColumnRef, value: CipherBlob) -> Predicate {
        Predicate {
            predicate_type: PredicateType::Lte,
            column: Some(column),
            value: Some(value),
            left: None,
            right: None,
        }
    }

    /// Create an AND predicate
    #[wasm_bindgen]
    pub fn and(left: Predicate, right: Predicate) -> Predicate {
        Predicate {
            predicate_type: PredicateType::And,
            column: None,
            value: None,
            left: Some(Box::new(left)),
            right: Some(Box::new(right)),
        }
    }

    /// Create an OR predicate
    #[wasm_bindgen]
    pub fn or(left: Predicate, right: Predicate) -> Predicate {
        Predicate {
            predicate_type: PredicateType::Or,
            column: None,
            value: None,
            left: Some(Box::new(left)),
            right: Some(Box::new(right)),
        }
    }

    /// Create a NOT predicate
    #[wasm_bindgen]
    #[allow(clippy::should_implement_trait)]
    pub fn not(predicate: Predicate) -> Predicate {
        Predicate {
            predicate_type: PredicateType::Not,
            column: None,
            value: None,
            left: Some(Box::new(predicate)),
            right: None,
        }
    }

    /// Get the predicate type
    #[wasm_bindgen(getter, js_name = predicateType)]
    pub fn predicate_type(&self) -> PredicateType {
        self.predicate_type
    }

    /// Get the column (if applicable)
    #[wasm_bindgen(getter)]
    pub fn column(&self) -> Option<ColumnRef> {
        self.column.clone()
    }

    /// Get the value (if applicable)
    #[wasm_bindgen(getter)]
    pub fn value(&self) -> Option<CipherBlob> {
        self.value.clone()
    }
}

/// Update operation for update queries
#[wasm_bindgen]
#[derive(Debug, Clone)]
pub struct UpdateOp {
    update_type: UpdateType,
    column: ColumnRef,
    value: CipherBlob,
}

#[wasm_bindgen]
impl UpdateOp {
    /// Create a set update operation
    #[wasm_bindgen(js_name = set)]
    pub fn set(column: ColumnRef, value: CipherBlob) -> UpdateOp {
        UpdateOp {
            update_type: UpdateType::Set,
            column,
            value,
        }
    }

    /// Create an add update operation (FHE addition)
    #[wasm_bindgen(js_name = add)]
    pub fn add(column: ColumnRef, value: CipherBlob) -> UpdateOp {
        UpdateOp {
            update_type: UpdateType::Add,
            column,
            value,
        }
    }

    /// Create a multiply update operation (FHE multiplication)
    #[wasm_bindgen(js_name = mul)]
    pub fn mul(column: ColumnRef, value: CipherBlob) -> UpdateOp {
        UpdateOp {
            update_type: UpdateType::Mul,
            column,
            value,
        }
    }

    /// Get the update type
    #[wasm_bindgen(getter, js_name = updateType)]
    pub fn update_type(&self) -> UpdateType {
        self.update_type
    }

    /// Get the column
    #[wasm_bindgen(getter)]
    pub fn column(&self) -> ColumnRef {
        self.column.clone()
    }

    /// Get the value
    #[wasm_bindgen(getter)]
    pub fn value(&self) -> CipherBlob {
        self.value.clone()
    }
}

/// Query object representing a database query
#[wasm_bindgen]
#[derive(Debug, Clone)]
pub struct Query {
    query_type: QueryType,
    collection: String,
    key: Option<Key>,
    value: Option<CipherBlob>,
    predicate: Option<Predicate>,
    updates: Vec<UpdateOp>,
    start_key: Option<Key>,
    end_key: Option<Key>,
}

#[wasm_bindgen]
impl Query {
    /// Get the query type
    #[wasm_bindgen(getter, js_name = queryType)]
    pub fn query_type(&self) -> String {
        match self.query_type {
            QueryType::Get => "get".to_string(),
            QueryType::Set => "set".to_string(),
            QueryType::Delete => "delete".to_string(),
            QueryType::Filter => "filter".to_string(),
            QueryType::Update => "update".to_string(),
            QueryType::Range => "range".to_string(),
        }
    }

    /// Get the collection name
    #[wasm_bindgen(getter)]
    pub fn collection(&self) -> Option<String> {
        Some(self.collection.clone())
    }

    /// Get the key (if applicable)
    #[wasm_bindgen(getter)]
    pub fn key(&self) -> Option<Key> {
        self.key.clone()
    }

    /// Get the value (if applicable)
    #[wasm_bindgen(getter)]
    pub fn value(&self) -> Option<CipherBlob> {
        self.value.clone()
    }

    /// Get the predicate (if applicable)
    #[wasm_bindgen(getter)]
    pub fn predicate(&self) -> Option<Predicate> {
        self.predicate.clone()
    }

    /// Get the start key (for range queries)
    #[wasm_bindgen(getter, js_name = startKey)]
    pub fn start_key(&self) -> Option<Key> {
        self.start_key.clone()
    }

    /// Get the end key (for range queries)
    #[wasm_bindgen(getter, js_name = endKey)]
    pub fn end_key(&self) -> Option<Key> {
        self.end_key.clone()
    }

    /// Get the number of updates
    #[wasm_bindgen(getter, js_name = updateCount)]
    pub fn update_count(&self) -> usize {
        self.updates.len()
    }

    /// Get an update at index
    #[wasm_bindgen(js_name = getUpdate)]
    pub fn get_update(&self, index: usize) -> Option<UpdateOp> {
        self.updates.get(index).cloned()
    }
}

/// Fluent query builder for TypeScript
///
/// # Example (TypeScript)
///
/// ```typescript
/// // Simple get query
/// const query = new QueryBuilder('users').get(Key.fromString('user:123'));
///
/// // Set query
/// const query = new QueryBuilder('users')
///     .set(Key.fromString('user:123'), CipherBlob.fromBytes(data));
///
/// // Filter query with predicate
/// const query = new QueryBuilder('users')
///     .whereClause()
///     .eq(col('status'), CipherBlob.fromBytes(statusData))
///     .build();
///
/// // Range query
/// const query = new QueryBuilder('data')
///     .range(Key.fromString('a'), Key.fromString('z'));
/// ```
#[wasm_bindgen]
pub struct QueryBuilder {
    collection: String,
}

#[wasm_bindgen]
impl QueryBuilder {
    /// Create a new query builder for a collection
    #[wasm_bindgen(constructor)]
    pub fn new(collection: &str) -> Self {
        Self {
            collection: collection.to_string(),
        }
    }

    /// Build a Get query
    #[wasm_bindgen]
    pub fn get(self, key: Key) -> Query {
        Query {
            query_type: QueryType::Get,
            collection: self.collection,
            key: Some(key),
            value: None,
            predicate: None,
            updates: Vec::new(),
            start_key: None,
            end_key: None,
        }
    }

    /// Build a Set query
    #[wasm_bindgen]
    pub fn set(self, key: Key, value: CipherBlob) -> Query {
        Query {
            query_type: QueryType::Set,
            collection: self.collection,
            key: Some(key),
            value: Some(value),
            predicate: None,
            updates: Vec::new(),
            start_key: None,
            end_key: None,
        }
    }

    /// Build a Delete query
    #[wasm_bindgen]
    pub fn delete(self, key: Key) -> Query {
        Query {
            query_type: QueryType::Delete,
            collection: self.collection,
            key: Some(key),
            value: None,
            predicate: None,
            updates: Vec::new(),
            start_key: None,
            end_key: None,
        }
    }

    /// Build a Filter query with a predicate
    #[wasm_bindgen]
    pub fn filter(self, predicate: Predicate) -> Query {
        Query {
            query_type: QueryType::Filter,
            collection: self.collection,
            key: None,
            value: None,
            predicate: Some(predicate),
            updates: Vec::new(),
            start_key: None,
            end_key: None,
        }
    }

    /// Build a Range query
    #[wasm_bindgen]
    pub fn range(self, start: Key, end: Key) -> Query {
        Query {
            query_type: QueryType::Range,
            collection: self.collection,
            key: None,
            value: None,
            predicate: None,
            updates: Vec::new(),
            start_key: Some(start),
            end_key: Some(end),
        }
    }

    /// Start building a where clause
    #[wasm_bindgen(js_name = whereClause)]
    pub fn where_clause(self) -> PredicateBuilder {
        PredicateBuilder::new(&self.collection)
    }
}

/// Predicate builder for constructing complex predicates
#[wasm_bindgen]
pub struct PredicateBuilder {
    collection: String,
}

#[wasm_bindgen]
impl PredicateBuilder {
    /// Create a new predicate builder
    #[wasm_bindgen(constructor)]
    pub fn new(collection: &str) -> Self {
        Self {
            collection: collection.to_string(),
        }
    }

    /// Create an equality filter and continue building
    #[wasm_bindgen]
    pub fn eq(self, column: ColumnRef, value: CipherBlob) -> FilterBuilder {
        FilterBuilder::new(self.collection, Predicate::eq(column, value))
    }

    /// Create a greater than filter and continue building
    #[wasm_bindgen]
    pub fn gt(self, column: ColumnRef, value: CipherBlob) -> FilterBuilder {
        FilterBuilder::new(self.collection, Predicate::gt(column, value))
    }

    /// Create a less than filter and continue building
    #[wasm_bindgen]
    pub fn lt(self, column: ColumnRef, value: CipherBlob) -> FilterBuilder {
        FilterBuilder::new(self.collection, Predicate::lt(column, value))
    }

    /// Create a greater than or equal filter and continue building
    #[wasm_bindgen]
    pub fn gte(self, column: ColumnRef, value: CipherBlob) -> FilterBuilder {
        FilterBuilder::new(self.collection, Predicate::gte(column, value))
    }

    /// Create a less than or equal filter and continue building
    #[wasm_bindgen]
    pub fn lte(self, column: ColumnRef, value: CipherBlob) -> FilterBuilder {
        FilterBuilder::new(self.collection, Predicate::lte(column, value))
    }
}

/// Filter builder for combining predicates
#[wasm_bindgen]
pub struct FilterBuilder {
    collection: String,
    predicate: Predicate,
}

#[wasm_bindgen]
impl FilterBuilder {
    /// Create a new filter builder
    fn new(collection: String, predicate: Predicate) -> Self {
        Self {
            collection,
            predicate,
        }
    }

    /// Add an AND condition
    #[wasm_bindgen]
    pub fn and(mut self, other: Predicate) -> Self {
        self.predicate = Predicate::and(self.predicate, other);
        self
    }

    /// Add an OR condition
    #[wasm_bindgen]
    pub fn or(mut self, other: Predicate) -> Self {
        self.predicate = Predicate::or(self.predicate, other);
        self
    }

    /// Negate the current predicate
    #[wasm_bindgen]
    #[allow(clippy::should_implement_trait)]
    pub fn not(mut self) -> Self {
        self.predicate = Predicate::not(self.predicate);
        self
    }

    /// Build the filter query
    #[wasm_bindgen]
    pub fn build(self) -> Query {
        Query {
            query_type: QueryType::Filter,
            collection: self.collection,
            key: None,
            value: None,
            predicate: Some(self.predicate),
            updates: Vec::new(),
            start_key: None,
            end_key: None,
        }
    }

    /// Build an update query with the current predicate
    #[wasm_bindgen]
    pub fn update(self, updates: Vec<UpdateOp>) -> Query {
        Query {
            query_type: QueryType::Update,
            collection: self.collection,
            key: None,
            value: None,
            predicate: Some(self.predicate),
            updates,
            start_key: None,
            end_key: None,
        }
    }

    /// Get the current predicate
    #[wasm_bindgen(getter)]
    pub fn predicate(&self) -> Predicate {
        self.predicate.clone()
    }
}

/// Helper function to create a query builder
#[wasm_bindgen]
pub fn query(collection: &str) -> QueryBuilder {
    QueryBuilder::new(collection)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_builder_get() {
        let q = QueryBuilder::new("users").get(Key::from_string("user:123"));
        assert_eq!(q.query_type(), "get");
        assert_eq!(q.collection(), Some("users".to_string()));
        assert!(q.key().is_some());
    }

    #[test]
    fn test_query_builder_set() {
        let q = QueryBuilder::new("users").set(
            Key::from_string("user:123"),
            CipherBlob::from_bytes(&[1, 2, 3]),
        );
        assert_eq!(q.query_type(), "set");
        assert!(q.key().is_some());
        assert!(q.value().is_some());
    }

    #[test]
    fn test_query_builder_delete() {
        let q = QueryBuilder::new("users").delete(Key::from_string("user:123"));
        assert_eq!(q.query_type(), "delete");
    }

    #[test]
    fn test_query_builder_range() {
        let q = QueryBuilder::new("data").range(Key::from_string("a"), Key::from_string("z"));
        assert_eq!(q.query_type(), "range");
        assert!(q.start_key().is_some());
        assert!(q.end_key().is_some());
    }

    #[test]
    fn test_predicate_eq() {
        let pred = Predicate::eq(ColumnRef::new("status"), CipherBlob::from_bytes(&[1]));
        assert_eq!(pred.predicate_type(), PredicateType::Eq);
        assert!(pred.column().is_some());
        assert!(pred.value().is_some());
    }

    #[test]
    fn test_predicate_and() {
        let left = Predicate::eq(ColumnRef::new("a"), CipherBlob::from_bytes(&[1]));
        let right = Predicate::gt(ColumnRef::new("b"), CipherBlob::from_bytes(&[2]));
        let combined = Predicate::and(left, right);
        assert_eq!(combined.predicate_type(), PredicateType::And);
    }

    #[test]
    fn test_predicate_or() {
        let left = Predicate::eq(ColumnRef::new("a"), CipherBlob::from_bytes(&[1]));
        let right = Predicate::lt(ColumnRef::new("b"), CipherBlob::from_bytes(&[2]));
        let combined = Predicate::or(left, right);
        assert_eq!(combined.predicate_type(), PredicateType::Or);
    }

    #[test]
    fn test_predicate_not() {
        let pred = Predicate::eq(ColumnRef::new("a"), CipherBlob::from_bytes(&[1]));
        let negated = Predicate::not(pred);
        assert_eq!(negated.predicate_type(), PredicateType::Not);
    }

    #[test]
    fn test_filter_builder() {
        let q = QueryBuilder::new("users")
            .where_clause()
            .eq(ColumnRef::new("status"), CipherBlob::from_bytes(&[1]))
            .build();
        assert_eq!(q.query_type(), "filter");
        assert!(q.predicate().is_some());
    }

    #[test]
    fn test_filter_builder_complex() {
        let q = QueryBuilder::new("users")
            .where_clause()
            .eq(ColumnRef::new("status"), CipherBlob::from_bytes(&[1]))
            .and(Predicate::gt(
                ColumnRef::new("age"),
                CipherBlob::from_bytes(&[18]),
            ))
            .or(Predicate::eq(
                ColumnRef::new("admin"),
                CipherBlob::from_bytes(&[1]),
            ))
            .build();

        assert_eq!(q.query_type(), "filter");
        assert!(q.predicate().is_some());
    }

    #[test]
    fn test_update_op() {
        let update = UpdateOp::set(ColumnRef::new("status"), CipherBlob::from_bytes(&[2]));
        assert_eq!(update.update_type(), UpdateType::Set);

        let update = UpdateOp::add(ColumnRef::new("counter"), CipherBlob::from_bytes(&[1]));
        assert_eq!(update.update_type(), UpdateType::Add);

        let update = UpdateOp::mul(ColumnRef::new("factor"), CipherBlob::from_bytes(&[2]));
        assert_eq!(update.update_type(), UpdateType::Mul);
    }

    #[test]
    fn test_query_helper() {
        let q = query("users").get(Key::from_string("test"));
        assert_eq!(q.query_type(), "get");
        assert_eq!(q.collection(), Some("users".to_string()));
    }
}
