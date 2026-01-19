//! Conversions between protocol buffer types and core types

use crate::error::{NetError, NetResult};
use crate::proto::{aql, errors, query, types};
use amaters_core::types::Update;
use amaters_core::types::{CipherBlob, CipherMetadata, CompressionType};
use amaters_core::{Key, Predicate, Query};

/// Convert core CipherBlob to proto CipherBlob
pub fn cipher_blob_to_proto(blob: &CipherBlob) -> types::CipherBlob {
    let metadata = blob.metadata();
    let proto_metadata = types::CipherMetadata {
        size: metadata.size as u64,
        compression: compression_type_to_proto(metadata.compression) as i32,
        checksum: metadata.checksum,
        created_at: metadata.created_at.timestamp(),
        version: metadata.version,
    };

    types::CipherBlob {
        data: blob.as_bytes().to_vec(),
        metadata: Some(proto_metadata),
    }
}

/// Convert proto CipherBlob to core CipherBlob
pub fn cipher_blob_from_proto(proto: types::CipherBlob) -> NetResult<CipherBlob> {
    let metadata = proto
        .metadata
        .ok_or_else(|| NetError::MissingField("metadata".to_string()))?;

    let core_metadata = CipherMetadata {
        size: metadata.size as usize,
        compression: compression_type_from_proto(metadata.compression)?,
        checksum: metadata.checksum,
        created_at: chrono::DateTime::from_timestamp(metadata.created_at, 0)
            .ok_or_else(|| NetError::MalformedMessage("invalid timestamp".to_string()))?,
        version: metadata.version,
    };

    Ok(CipherBlob::with_metadata(proto.data, core_metadata))
}

/// Convert core CompressionType to proto CompressionType
fn compression_type_to_proto(compression: CompressionType) -> types::CompressionType {
    match compression {
        CompressionType::None => types::CompressionType::CompressionNone,
        CompressionType::Lz4 => types::CompressionType::CompressionLz4,
        CompressionType::Zstd => types::CompressionType::CompressionZstd,
    }
}

/// Convert proto CompressionType to core CompressionType
fn compression_type_from_proto(compression: i32) -> NetResult<CompressionType> {
    match types::CompressionType::try_from(compression) {
        Ok(types::CompressionType::CompressionNone) => Ok(CompressionType::None),
        Ok(types::CompressionType::CompressionLz4) => Ok(CompressionType::Lz4),
        Ok(types::CompressionType::CompressionZstd) => Ok(CompressionType::Zstd),
        _ => Err(NetError::MalformedMessage(
            "invalid compression type".to_string(),
        )),
    }
}

/// Convert core Key to proto Key
pub fn key_to_proto(key: &Key) -> types::Key {
    types::Key {
        data: key.as_bytes().to_vec(),
    }
}

/// Convert proto Key to core Key
pub fn key_from_proto(proto: types::Key) -> Key {
    Key::from_slice(&proto.data)
}

/// Convert core Query to proto Query
pub fn query_to_proto(query: &Query) -> NetResult<query::Query> {
    let query_enum = match query {
        Query::Get { collection, key } => query::query::Query::Get(query::GetQuery {
            collection: collection.clone(),
            key: Some(key_to_proto(key)),
        }),
        Query::Set {
            collection,
            key,
            value,
        } => query::query::Query::Set(query::SetQuery {
            collection: collection.clone(),
            key: Some(key_to_proto(key)),
            value: Some(cipher_blob_to_proto(value)),
        }),
        Query::Delete { collection, key } => query::query::Query::Delete(query::DeleteQuery {
            collection: collection.clone(),
            key: Some(key_to_proto(key)),
        }),
        Query::Filter {
            collection,
            predicate,
        } => query::query::Query::Filter(query::FilterQuery {
            collection: collection.clone(),
            predicate: Some(predicate_to_proto(predicate)?),
            limit: None,
            offset: None,
        }),
        Query::Update {
            collection,
            predicate,
            updates,
        } => query::query::Query::Update(query::UpdateQuery {
            collection: collection.clone(),
            predicate: Some(predicate_to_proto(predicate)?),
            updates: updates
                .iter()
                .map(update_to_proto)
                .collect::<NetResult<Vec<_>>>()?,
        }),
        Query::Range {
            collection,
            start,
            end,
        } => query::query::Query::Range(query::RangeQuery {
            collection: collection.clone(),
            start: Some(key_to_proto(start)),
            end: Some(key_to_proto(end)),
            limit: None,
        }),
    };

    Ok(query::Query {
        query: Some(query_enum),
    })
}

/// Convert proto Query to core Query
pub fn query_from_proto(proto: query::Query) -> NetResult<Query> {
    let query_enum = proto
        .query
        .ok_or_else(|| NetError::MissingField("query".to_string()))?;

    match query_enum {
        query::query::Query::Get(q) => {
            let key = q
                .key
                .ok_or_else(|| NetError::MissingField("key".to_string()))?;
            Ok(Query::Get {
                collection: q.collection,
                key: key_from_proto(key),
            })
        }
        query::query::Query::Set(q) => {
            let key = q
                .key
                .ok_or_else(|| NetError::MissingField("key".to_string()))?;
            let value = q
                .value
                .ok_or_else(|| NetError::MissingField("value".to_string()))?;
            Ok(Query::Set {
                collection: q.collection,
                key: key_from_proto(key),
                value: cipher_blob_from_proto(value)?,
            })
        }
        query::query::Query::Delete(q) => {
            let key = q
                .key
                .ok_or_else(|| NetError::MissingField("key".to_string()))?;
            Ok(Query::Delete {
                collection: q.collection,
                key: key_from_proto(key),
            })
        }
        query::query::Query::Filter(q) => {
            let predicate = q
                .predicate
                .ok_or_else(|| NetError::MissingField("predicate".to_string()))?;
            Ok(Query::Filter {
                collection: q.collection,
                predicate: predicate_from_proto(predicate)?,
            })
        }
        query::query::Query::Update(q) => {
            let predicate = q
                .predicate
                .ok_or_else(|| NetError::MissingField("predicate".to_string()))?;
            let updates = q
                .updates
                .into_iter()
                .map(update_from_proto)
                .collect::<NetResult<Vec<_>>>()?;
            Ok(Query::Update {
                collection: q.collection,
                predicate: predicate_from_proto(predicate)?,
                updates,
            })
        }
        query::query::Query::Range(q) => {
            let start = q
                .start
                .ok_or_else(|| NetError::MissingField("start".to_string()))?;
            let end = q
                .end
                .ok_or_else(|| NetError::MissingField("end".to_string()))?;
            Ok(Query::Range {
                collection: q.collection,
                start: key_from_proto(start),
                end: key_from_proto(end),
            })
        }
    }
}

/// Convert core Predicate to proto Predicate
fn predicate_to_proto(predicate: &Predicate) -> NetResult<types::Predicate> {
    let pred_enum = match predicate {
        Predicate::Eq(col, val) => types::predicate::Predicate::Eq(types::EqPredicate {
            column: Some(types::ColumnRef {
                name: col.name.clone(),
            }),
            value: Some(cipher_blob_to_proto(val)),
        }),
        Predicate::Gt(col, val) => types::predicate::Predicate::Gt(types::GtPredicate {
            column: Some(types::ColumnRef {
                name: col.name.clone(),
            }),
            value: Some(cipher_blob_to_proto(val)),
        }),
        Predicate::Lt(col, val) => types::predicate::Predicate::Lt(types::LtPredicate {
            column: Some(types::ColumnRef {
                name: col.name.clone(),
            }),
            value: Some(cipher_blob_to_proto(val)),
        }),
        Predicate::Gte(col, val) => types::predicate::Predicate::Gte(types::GtePredicate {
            column: Some(types::ColumnRef {
                name: col.name.clone(),
            }),
            value: Some(cipher_blob_to_proto(val)),
        }),
        Predicate::Lte(col, val) => types::predicate::Predicate::Lte(types::LtePredicate {
            column: Some(types::ColumnRef {
                name: col.name.clone(),
            }),
            value: Some(cipher_blob_to_proto(val)),
        }),
        Predicate::And(left, right) => {
            types::predicate::Predicate::And(Box::new(types::AndPredicate {
                left: Some(Box::new(predicate_to_proto(left)?)),
                right: Some(Box::new(predicate_to_proto(right)?)),
            }))
        }
        Predicate::Or(left, right) => {
            types::predicate::Predicate::Or(Box::new(types::OrPredicate {
                left: Some(Box::new(predicate_to_proto(left)?)),
                right: Some(Box::new(predicate_to_proto(right)?)),
            }))
        }
        Predicate::Not(pred) => types::predicate::Predicate::Not(Box::new(types::NotPredicate {
            predicate: Some(Box::new(predicate_to_proto(pred)?)),
        })),
    };

    Ok(types::Predicate {
        predicate: Some(pred_enum),
    })
}

/// Convert proto Predicate to core Predicate
fn predicate_from_proto(proto: types::Predicate) -> NetResult<Predicate> {
    let pred_enum = proto
        .predicate
        .ok_or_else(|| NetError::MissingField("predicate".to_string()))?;

    match pred_enum {
        types::predicate::Predicate::Eq(p) => {
            let col = p
                .column
                .ok_or_else(|| NetError::MissingField("column".to_string()))?;
            let val = p
                .value
                .ok_or_else(|| NetError::MissingField("value".to_string()))?;
            Ok(Predicate::Eq(
                amaters_core::ColumnRef::new(col.name),
                cipher_blob_from_proto(val)?,
            ))
        }
        types::predicate::Predicate::Gt(p) => {
            let col = p
                .column
                .ok_or_else(|| NetError::MissingField("column".to_string()))?;
            let val = p
                .value
                .ok_or_else(|| NetError::MissingField("value".to_string()))?;
            Ok(Predicate::Gt(
                amaters_core::ColumnRef::new(col.name),
                cipher_blob_from_proto(val)?,
            ))
        }
        types::predicate::Predicate::Lt(p) => {
            let col = p
                .column
                .ok_or_else(|| NetError::MissingField("column".to_string()))?;
            let val = p
                .value
                .ok_or_else(|| NetError::MissingField("value".to_string()))?;
            Ok(Predicate::Lt(
                amaters_core::ColumnRef::new(col.name),
                cipher_blob_from_proto(val)?,
            ))
        }
        types::predicate::Predicate::Gte(p) => {
            let col = p
                .column
                .ok_or_else(|| NetError::MissingField("column".to_string()))?;
            let val = p
                .value
                .ok_or_else(|| NetError::MissingField("value".to_string()))?;
            Ok(Predicate::Gte(
                amaters_core::ColumnRef::new(col.name),
                cipher_blob_from_proto(val)?,
            ))
        }
        types::predicate::Predicate::Lte(p) => {
            let col = p
                .column
                .ok_or_else(|| NetError::MissingField("column".to_string()))?;
            let val = p
                .value
                .ok_or_else(|| NetError::MissingField("value".to_string()))?;
            Ok(Predicate::Lte(
                amaters_core::ColumnRef::new(col.name),
                cipher_blob_from_proto(val)?,
            ))
        }
        types::predicate::Predicate::And(p) => {
            let left = p
                .left
                .ok_or_else(|| NetError::MissingField("left".to_string()))?;
            let right = p
                .right
                .ok_or_else(|| NetError::MissingField("right".to_string()))?;
            Ok(Predicate::And(
                Box::new(predicate_from_proto(*left)?),
                Box::new(predicate_from_proto(*right)?),
            ))
        }
        types::predicate::Predicate::Or(p) => {
            let left = p
                .left
                .ok_or_else(|| NetError::MissingField("left".to_string()))?;
            let right = p
                .right
                .ok_or_else(|| NetError::MissingField("right".to_string()))?;
            Ok(Predicate::Or(
                Box::new(predicate_from_proto(*left)?),
                Box::new(predicate_from_proto(*right)?),
            ))
        }
        types::predicate::Predicate::Not(p) => {
            let pred = p
                .predicate
                .ok_or_else(|| NetError::MissingField("predicate".to_string()))?;
            Ok(Predicate::Not(Box::new(predicate_from_proto(*pred)?)))
        }
    }
}

/// Convert core Update to proto Update
fn update_to_proto(update: &Update) -> NetResult<types::Update> {
    let update_enum = match update {
        Update::Set(col, val) => types::update::Operation::Set(types::SetUpdate {
            column: Some(types::ColumnRef {
                name: col.name.clone(),
            }),
            value: Some(cipher_blob_to_proto(val)),
        }),
        Update::Add(col, val) => types::update::Operation::Add(types::AddUpdate {
            column: Some(types::ColumnRef {
                name: col.name.clone(),
            }),
            value: Some(cipher_blob_to_proto(val)),
        }),
        Update::Mul(col, val) => types::update::Operation::Mul(types::MulUpdate {
            column: Some(types::ColumnRef {
                name: col.name.clone(),
            }),
            value: Some(cipher_blob_to_proto(val)),
        }),
    };

    Ok(types::Update {
        operation: Some(update_enum),
    })
}

/// Convert proto Update to core Update
fn update_from_proto(proto: types::Update) -> NetResult<Update> {
    let update_enum = proto
        .operation
        .ok_or_else(|| NetError::MissingField("operation".to_string()))?;

    match update_enum {
        types::update::Operation::Set(u) => {
            let col = u
                .column
                .ok_or_else(|| NetError::MissingField("column".to_string()))?;
            let val = u
                .value
                .ok_or_else(|| NetError::MissingField("value".to_string()))?;
            Ok(Update::Set(
                amaters_core::ColumnRef::new(col.name),
                cipher_blob_from_proto(val)?,
            ))
        }
        types::update::Operation::Add(u) => {
            let col = u
                .column
                .ok_or_else(|| NetError::MissingField("column".to_string()))?;
            let val = u
                .value
                .ok_or_else(|| NetError::MissingField("value".to_string()))?;
            Ok(Update::Add(
                amaters_core::ColumnRef::new(col.name),
                cipher_blob_from_proto(val)?,
            ))
        }
        types::update::Operation::Mul(u) => {
            let col = u
                .column
                .ok_or_else(|| NetError::MissingField("column".to_string()))?;
            let val = u
                .value
                .ok_or_else(|| NetError::MissingField("value".to_string()))?;
            Ok(Update::Mul(
                amaters_core::ColumnRef::new(col.name),
                cipher_blob_from_proto(val)?,
            ))
        }
    }
}

/// Create protocol version
pub fn create_version() -> types::Version {
    types::Version {
        major: crate::PROTOCOL_VERSION.0,
        minor: crate::PROTOCOL_VERSION.1,
        patch: crate::PROTOCOL_VERSION.2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_conversion() {
        let key = Key::from_str("test_key");
        let proto = key_to_proto(&key);
        let converted = key_from_proto(proto);
        assert_eq!(key, converted);
    }

    #[test]
    fn test_cipher_blob_conversion() {
        let blob = CipherBlob::new(vec![1, 2, 3, 4, 5]);
        let proto = cipher_blob_to_proto(&blob);
        let converted = cipher_blob_from_proto(proto).expect("conversion failed");
        assert_eq!(blob, converted);
    }

    #[test]
    fn test_compression_type_conversion() {
        assert_eq!(
            compression_type_to_proto(CompressionType::None),
            types::CompressionType::CompressionNone
        );
        assert_eq!(
            compression_type_to_proto(CompressionType::Lz4),
            types::CompressionType::CompressionLz4
        );
        assert_eq!(
            compression_type_to_proto(CompressionType::Zstd),
            types::CompressionType::CompressionZstd
        );
    }

    #[test]
    fn test_version_creation() {
        let version = create_version();
        assert_eq!(version.major, 0);
        assert_eq!(version.minor, 1);
        assert_eq!(version.patch, 0);
    }
}
