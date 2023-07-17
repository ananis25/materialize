// Copyright Materialize, Inc. and contributors. All rights reserved.
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of the Change Date specified in that file, in accordance with
// the Business Source License, use of this software will be governed
// by the Apache License, Version 2.0.

// BEGIN LINT CONFIG
// DO NOT EDIT. Automatically generated by bin/gen-lints.
// Have complaints about the noise? See the note in misc/python/materialize/cli/gen-lints.py first.
#![allow(clippy::style)]
#![allow(clippy::complexity)]
#![allow(clippy::large_enum_variant)]
#![allow(clippy::mutable_key_type)]
#![allow(clippy::stable_sort_primitive)]
#![allow(clippy::map_entry)]
#![allow(clippy::box_default)]
#![warn(clippy::bool_comparison)]
#![warn(clippy::clone_on_ref_ptr)]
#![warn(clippy::no_effect)]
#![warn(clippy::unnecessary_unwrap)]
#![warn(clippy::dbg_macro)]
#![warn(clippy::todo)]
#![warn(clippy::wildcard_dependencies)]
#![warn(clippy::zero_prefixed_literal)]
#![warn(clippy::borrowed_box)]
#![warn(clippy::deref_addrof)]
#![warn(clippy::double_must_use)]
#![warn(clippy::double_parens)]
#![warn(clippy::extra_unused_lifetimes)]
#![warn(clippy::needless_borrow)]
#![warn(clippy::needless_question_mark)]
#![warn(clippy::needless_return)]
#![warn(clippy::redundant_pattern)]
#![warn(clippy::redundant_slicing)]
#![warn(clippy::redundant_static_lifetimes)]
#![warn(clippy::single_component_path_imports)]
#![warn(clippy::unnecessary_cast)]
#![warn(clippy::useless_asref)]
#![warn(clippy::useless_conversion)]
#![warn(clippy::builtin_type_shadow)]
#![warn(clippy::duplicate_underscore_argument)]
#![warn(clippy::double_neg)]
#![warn(clippy::unnecessary_mut_passed)]
#![warn(clippy::wildcard_in_or_patterns)]
#![warn(clippy::crosspointer_transmute)]
#![warn(clippy::excessive_precision)]
#![warn(clippy::overflow_check_conditional)]
#![warn(clippy::as_conversions)]
#![warn(clippy::match_overlapping_arm)]
#![warn(clippy::zero_divided_by_zero)]
#![warn(clippy::must_use_unit)]
#![warn(clippy::suspicious_assignment_formatting)]
#![warn(clippy::suspicious_else_formatting)]
#![warn(clippy::suspicious_unary_op_formatting)]
#![warn(clippy::mut_mutex_lock)]
#![warn(clippy::print_literal)]
#![warn(clippy::same_item_push)]
#![warn(clippy::useless_format)]
#![warn(clippy::write_literal)]
#![warn(clippy::redundant_closure)]
#![warn(clippy::redundant_closure_call)]
#![warn(clippy::unnecessary_lazy_evaluations)]
#![warn(clippy::partialeq_ne_impl)]
#![warn(clippy::redundant_field_names)]
#![warn(clippy::transmutes_expressible_as_ptr_casts)]
#![warn(clippy::unused_async)]
#![warn(clippy::disallowed_methods)]
#![warn(clippy::disallowed_macros)]
#![warn(clippy::disallowed_types)]
#![warn(clippy::from_over_into)]
// END LINT CONFIG

//! Durable metadata storage.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{self, Debug};
use std::marker::PhantomData;
use std::sync::Arc;

use mz_ore::soft_assert;
use mz_proto::{RustType, TryFromProtoError};
use serde::{Deserialize, Serialize};
use timely::progress::Antichain;

pub mod objects;

mod postgres;
mod transaction;

// TODO(parkmycar): This shouldn't be public, but it is for now to prevent dead code warnings.
pub mod upgrade;

pub use crate::postgres::{DebugStashFactory, Stash, StashFactory};
pub use crate::transaction::{Transaction, INSERT_BATCH_SPLIT_SIZE};

/// The current version of the [`Stash`].
///
/// We will initialize new [`Stash`]es with this version, and migrate existing [`Stash`]es to this
/// version. Whenever the [`Stash`] changes, e.g. the protobufs we serialize in the [`Stash`]
/// change, we need to bump this version.
pub const STASH_VERSION: u64 = 28;

/// The minimum [`Stash`] version number that we support migrating from.
///
/// After bumping this we can delete the old migrations.
pub(crate) const MIN_STASH_VERSION: u64 = 13;

// TODO(jkosh44) There's some circular logic going on with this key across crates.
// mz_adapter::catalog::storage::stash initializes uses this value to initialize
// `CONFIG_COLLECTION: mz_stash::TypedCollection`. Then `mz_stash::postgres::Stash` uses this value
// to check if the stash has been initialized.
/// The key within the Config Collection that stores the version of the Stash.
pub const USER_VERSION_KEY: &str = "user_version";
pub const COLLECTION_CONFIG: TypedCollection<
    objects::proto::ConfigKey,
    objects::proto::ConfigValue,
> = TypedCollection::new("config");

pub type Diff = i64;
pub type Timestamp = i64;

pub(crate) type Id = i64;

/// A common trait for uses of K and V to express in a single place all of the
/// traits required by async_trait and StashCollection.
pub trait Data:
    prost::Message + Default + Ord + Send + Sync + Serialize + for<'de> Deserialize<'de>
{
}
impl<T: prost::Message + Default + Ord + Send + Sync + Serialize + for<'de> Deserialize<'de>> Data
    for T
{
}

/// `StashCollection` is like a differential dataflow [`Collection`], but the
/// state of the collection is durable.
///
/// A `StashCollection` stores `(key, value, timestamp, diff)` entries. The key
/// and value types are chosen by the caller; they must implement [`Ord`] and
/// they must be serializable to and deserializable via serde. The timestamp and
/// diff types are fixed to `i64`.
///
/// A `StashCollection` maintains a since frontier and an upper frontier, as
/// described in the [correctness vocabulary document]. To advance the since
/// frontier, call [`compact`]. To advance the upper frontier, call [`seal`].
///
/// [`compact`]: Transaction::compact
/// [`seal`]: Transaction::seal
/// [correctness vocabulary document]: https://github.com/MaterializeInc/materialize/blob/main/doc/developer/design/20210831_correctness.md
/// [`Collection`]: differential_dataflow::collection::Collection
#[derive(Debug)]
pub struct StashCollection<K, V> {
    pub id: Id,
    _kv: PhantomData<(K, V)>,
}

impl<K, V> StashCollection<K, V> {
    fn new(id: Id) -> Self {
        Self {
            id,
            _kv: PhantomData,
        }
    }
}

impl<K, V> Clone for StashCollection<K, V> {
    fn clone(&self) -> Self {
        Self::new(self.id)
    }
}

impl<K, V> Copy for StashCollection<K, V> {}

struct AntichainFormatter<'a, T>(&'a [T]);

impl<T> fmt::Display for AntichainFormatter<'_, T>
where
    T: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("{")?;
        for (i, element) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            element.fmt(f)?;
        }
        f.write_str("}")
    }
}

impl<'a, T> From<&'a Antichain<T>> for AntichainFormatter<'a, T> {
    fn from(antichain: &Antichain<T>) -> AntichainFormatter<T> {
        AntichainFormatter(antichain.elements())
    }
}

/// An error that can occur while interacting with a [`Stash`].
///
/// Stash errors are deliberately opaque. They generally indicate unrecoverable
/// conditions, like running out of disk space.
#[derive(Debug)]
pub struct StashError {
    // Internal to avoid leaking implementation details.
    inner: InternalStashError,
}

impl StashError {
    /// Reports whether the error is unrecoverable (retrying will never succeed,
    /// or a retry is not safe due to an indeterminate state).
    pub fn is_unrecoverable(&self) -> bool {
        match &self.inner {
            InternalStashError::Fence(_) => true,
            _ => false,
        }
    }

    /// Reports whether the error can be recovered if we opened the stash in writeable
    pub fn can_recover_with_write_mode(&self) -> bool {
        match &self.inner {
            InternalStashError::StashNotWritable(_) => true,
            _ => false,
        }
    }
}

#[derive(Debug)]
enum InternalStashError {
    Postgres(::tokio_postgres::Error),
    Fence(String),
    PeekSinceUpper(String),
    IncompatibleVersion(u64),
    Proto(TryFromProtoError),
    Decoding(prost::DecodeError),
    Uninitialized,
    StashNotWritable(String),
    Other(String),
}

impl fmt::Display for StashError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("stash error: ")?;
        match &self.inner {
            InternalStashError::Postgres(e) => write!(f, "postgres: {e}"),
            InternalStashError::Proto(e) => write!(f, "proto: {e}"),
            InternalStashError::Decoding(e) => write!(f, "prost decoding: {e}"),
            InternalStashError::Fence(e) => f.write_str(e),
            InternalStashError::PeekSinceUpper(e) => f.write_str(e),
            InternalStashError::IncompatibleVersion(v) => {
                write!(f, "incompatible Stash version {v}, minimum: {MIN_STASH_VERSION}, current: {STASH_VERSION}")
            }
            InternalStashError::Uninitialized => write!(f, "uninitialized"),
            InternalStashError::StashNotWritable(e) => f.write_str(e),
            InternalStashError::Other(e) => f.write_str(e),
        }
    }
}

impl Error for StashError {}

impl From<InternalStashError> for StashError {
    fn from(inner: InternalStashError) -> StashError {
        StashError { inner }
    }
}

impl From<prost::DecodeError> for StashError {
    fn from(e: prost::DecodeError) -> Self {
        StashError {
            inner: InternalStashError::Decoding(e),
        }
    }
}

impl From<TryFromProtoError> for StashError {
    fn from(e: TryFromProtoError) -> Self {
        StashError {
            inner: InternalStashError::Proto(e),
        }
    }
}

impl From<String> for StashError {
    fn from(e: String) -> StashError {
        StashError {
            inner: InternalStashError::Other(e),
        }
    }
}

impl From<&str> for StashError {
    fn from(e: &str) -> StashError {
        StashError {
            inner: InternalStashError::Other(e.into()),
        }
    }
}

impl From<std::io::Error> for StashError {
    fn from(e: std::io::Error) -> StashError {
        StashError {
            inner: InternalStashError::Other(e.to_string()),
        }
    }
}

impl From<anyhow::Error> for StashError {
    fn from(e: anyhow::Error) -> Self {
        StashError {
            inner: InternalStashError::Other(e.to_string()),
        }
    }
}

/// An [`AppendBatch`] describes a set of changes to append to a stash collection.
#[derive(Clone, Debug)]
pub struct AppendBatch {
    /// Id of stash collection.
    pub(crate) collection_id: Id,
    /// Current upper of the collection. The collection will also be compacted to `lower`.
    pub(crate) lower: Antichain<Timestamp>,
    /// The collection will be sealed to `upper`.
    pub upper: Antichain<Timestamp>,
    /// The timestamp of all entries in `entries`.
    pub(crate) timestamp: Timestamp,
    /// Entries to append to a collection. Each entry is of the form
    /// ((key [in bytes], value [in bytes]), timestamp, diff).
    pub(crate) entries: Vec<((Vec<u8>, Vec<u8>), Timestamp, Diff)>,
}

impl<K, V> StashCollection<K, V> {
    /// Create a new AppendBatch for this collection from its current upper.
    pub async fn make_batch_tx(&self, tx: &Transaction<'_>) -> Result<AppendBatch, StashError> {
        let id = self.id;
        let lower = tx.upper(id).await?;
        self.make_batch_lower(lower)
    }

    /// Create a new AppendBatch for this collection from its current upper.
    pub fn make_batch_lower(&self, lower: Antichain<Timestamp>) -> Result<AppendBatch, StashError> {
        let timestamp: Timestamp = match lower.elements() {
            [ts] => *ts,
            _ => return Err("cannot determine batch timestamp".into()),
        };
        let upper = match timestamp.checked_add(1) {
            Some(ts) => Antichain::from_elem(ts),
            None => return Err("cannot determine new upper".into()),
        };
        Ok(AppendBatch {
            collection_id: self.id,
            lower,
            upper,
            timestamp,
            entries: Vec::new(),
        })
    }
}

impl<K, V> StashCollection<K, V>
where
    K: Data,
    V: Data,
{
    /// Append `key`, `value`, `diff` to `batch`.
    pub fn append_to_batch(&self, batch: &mut AppendBatch, key: &K, value: &V, diff: Diff) {
        let key = key.encode_to_vec();
        let val = value.encode_to_vec();
        batch.entries.push(((key, val), batch.timestamp, diff));
    }
}

impl<K, V> From<Id> for StashCollection<K, V> {
    fn from(id: Id) -> Self {
        Self {
            id,
            _kv: PhantomData,
        }
    }
}

/// A helper struct to prevent mistyping of a [`StashCollection`]'s name and
/// k,v types.
pub struct TypedCollection<K, V> {
    name: &'static str,
    typ: PhantomData<(K, V)>,
}

impl<K, V> TypedCollection<K, V> {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            typ: PhantomData,
        }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }
}

impl<K, V> TypedCollection<K, V>
where
    K: Data,
    V: Data,
{
    pub async fn from_tx(&self, tx: &Transaction<'_>) -> Result<StashCollection<K, V>, StashError> {
        tx.collection(self.name).await
    }

    pub async fn iter(
        &self,
        stash: &mut Stash,
    ) -> Result<Vec<((K, V), Timestamp, Diff)>, StashError> {
        let name = self.name;
        stash
            .with_transaction(move |tx| {
                Box::pin(async move {
                    let collection = tx.collection::<K, V>(name).await?;
                    tx.iter(collection).await
                })
            })
            .await
    }

    pub async fn peek_one(&self, stash: &mut Stash) -> Result<BTreeMap<K, V>, StashError> {
        let name = self.name;
        stash
            .with_transaction(move |tx| {
                Box::pin(async move {
                    let collection = tx.collection::<K, V>(name).await?;
                    tx.peek_one(collection).await
                })
            })
            .await
    }

    #[tracing::instrument(level = "trace", skip_all)]
    pub async fn peek_key_one(&self, stash: &mut Stash, key: K) -> Result<Option<V>, StashError>
    where
        // TODO: Is it possible to remove the 'static?
        K: 'static,
    {
        let name = self.name;
        let key = Arc::new(key);
        stash
            .with_transaction(move |tx| {
                Box::pin(async move {
                    let collection = tx.collection::<K, V>(name).await?;
                    tx.peek_key_one(collection, &key).await
                })
            })
            .await
    }

    /// Sets the given k,v pair, if not already set.
    ///
    /// Returns the new value stored in stash after this operations.
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn insert_key_without_overwrite(
        &self,
        stash: &mut Stash,
        key: K,
        value: V,
    ) -> Result<V, StashError>
    where
        // TODO: Is it possible to remove the 'statics?
        K: 'static,
        V: Clone + 'static,
    {
        let name = self.name;
        let key = Arc::new(key);
        let value = Arc::new(value);
        stash
            .with_transaction(move |tx| {
                Box::pin(async move {
                    let collection = tx.collection::<K, V>(name).await?;
                    let lower = tx.upper(collection.id).await?;
                    let mut batch = collection.make_batch_lower(lower)?;
                    let prev = match tx.peek_key_one(collection, &key).await {
                        Ok(prev) => prev,
                        Err(err) => match err.inner {
                            InternalStashError::PeekSinceUpper(_) => {
                                // If the upper isn't > since, bump the upper and try again to find a sealed
                                // entry. Do this by appending the empty batch which will advance the upper.
                                tx.append(vec![batch]).await?;
                                let lower = tx.upper(collection.id).await?;
                                batch = collection.make_batch_lower(lower)?;
                                tx.peek_key_one(collection, &key).await?
                            }
                            _ => return Err(err),
                        },
                    };
                    match prev {
                        Some(prev) => Ok(prev),
                        None => {
                            collection.append_to_batch(&mut batch, &key, &value, 1);
                            tx.append(vec![batch]).await?;
                            Ok((*value).clone())
                        }
                    }
                })
            })
            .await
    }

    /// Sets the given key value pairs, if not already set. If a new key appears
    /// multiple times in `entries`, its value will be from the first occurrence
    /// in `entries`.
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn insert_without_overwrite<I>(
        &self,
        stash: &mut Stash,
        entries: I,
    ) -> Result<(), StashError>
    where
        I: IntoIterator<Item = (K, V)>,
        // TODO: Figure out if it's possible to remove the 'static bounds.
        K: Clone + 'static,
        V: Clone + 'static,
    {
        let name = self.name;
        let entries: Vec<_> = entries.into_iter().collect();
        let entries = Arc::new(entries);
        stash
            .with_transaction(move |tx| {
                Box::pin(async move {
                    let collection = tx.collection::<K, V>(name).await?;
                    let lower = tx.upper(collection.id).await?;
                    let mut batch = collection.make_batch_lower(lower)?;
                    let mut prev = match tx.peek_one(collection).await {
                        Ok(prev) => prev,
                        Err(err) => match err.inner {
                            InternalStashError::PeekSinceUpper(_) => {
                                // If the upper isn't > since, bump the upper and try again to find a sealed
                                // entry. Do this by appending the empty batch which will advance the upper.
                                tx.append(vec![batch]).await?;
                                let lower = tx.upper(collection.id).await?;
                                batch = collection.make_batch_lower(lower)?;
                                tx.peek_one(collection).await?
                            }
                            _ => return Err(err),
                        },
                    };
                    for (k, v) in entries.iter() {
                        if !prev.contains_key(k) {
                            collection.append_to_batch(&mut batch, k, v, 1);
                            prev.insert(k.clone(), v.clone());
                        }
                    }
                    tx.append(vec![batch]).await?;
                    Ok(())
                })
            })
            .await
    }

    /// Sets a value for a key. `f` is passed the previous value, if any.
    ///
    /// Returns the previous value if one existed and the value returned from
    /// `f`.
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn upsert_key<F, R>(
        &self,
        stash: &mut Stash,
        key: K,
        f: F,
    ) -> Result<Result<(Option<V>, V), R>, StashError>
    where
        F: FnOnce(Option<&V>) -> Result<V, R> + Clone + Send + Sync + 'static,
        K: 'static,
    {
        let name = self.name;
        let key = Arc::new(key);
        stash
            .with_transaction(move |tx| {
                Box::pin(async move {
                    let collection = tx.collection::<K, V>(name).await?;
                    let lower = tx.upper(collection.id).await?;
                    let mut batch = collection.make_batch_lower(lower)?;
                    let prev = match tx.peek_key_one(collection, &key).await {
                        Ok(prev) => prev,
                        Err(err) => match err.inner {
                            InternalStashError::PeekSinceUpper(_) => {
                                // If the upper isn't > since, bump the upper and try again to find a sealed
                                // entry. Do this by appending the empty batch which will advance the upper.
                                tx.append(vec![batch]).await?;
                                let lower = tx.upper(collection.id).await?;
                                batch = collection.make_batch_lower(lower)?;
                                tx.peek_key_one(collection, &key).await?
                            }
                            _ => return Err(err),
                        },
                    };
                    let next = match f(prev.as_ref()) {
                        Ok(v) => v,
                        Err(e) => return Ok(Err(e)),
                    };
                    // Do nothing if the values are the same.
                    if Some(&next) != prev.as_ref() {
                        if let Some(prev) = &prev {
                            collection.append_to_batch(&mut batch, &key, prev, -1);
                        }
                        collection.append_to_batch(&mut batch, &key, &next, 1);
                        tx.append(vec![batch]).await?;
                    }
                    Ok(Ok((prev, next)))
                })
            })
            .await
    }

    /// Sets the given key value pairs, removing existing entries match any key.
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn upsert<I>(&self, stash: &mut Stash, entries: I) -> Result<(), StashError>
    where
        I: IntoIterator<Item = (K, V)>,
        K: 'static,
        V: 'static,
    {
        let name = self.name;
        let entries: Vec<_> = entries.into_iter().collect();
        let entries = Arc::new(entries);
        stash
            .with_transaction(move |tx| {
                Box::pin(async move {
                    let collection = tx.collection::<K, V>(name).await?;
                    let lower = tx.upper(collection.id).await?;
                    let mut batch = collection.make_batch_lower(lower)?;
                    let prev = match tx.peek_one(collection).await {
                        Ok(prev) => prev,
                        Err(err) => match err.inner {
                            InternalStashError::PeekSinceUpper(_) => {
                                // If the upper isn't > since, bump the upper and try again to find a sealed
                                // entry. Do this by appending the empty batch which will advance the upper.
                                tx.append(vec![batch]).await?;
                                let lower = tx.upper(collection.id).await?;
                                batch = collection.make_batch_lower(lower)?;
                                tx.peek_one(collection).await?
                            }
                            _ => return Err(err),
                        },
                    };
                    for (k, v) in entries.iter() {
                        if let Some(prev_v) = prev.get(k) {
                            collection.append_to_batch(&mut batch, k, prev_v, -1);
                        }
                        collection.append_to_batch(&mut batch, k, v, 1);
                    }
                    tx.append(vec![batch]).await?;
                    Ok(())
                })
            })
            .await
    }

    /// Transactionally deletes any kv pair from `self` whose key is in `keys`.
    ///
    /// Note that:
    /// - Unlike `delete`, this operation operates in time O(keys), and not
    ///   O(set), however does so by parallelizing a number of point queries so
    ///   is likely not performant for more than 10-or-so keys.
    /// - This operation runs in a single transaction and cannot be combined
    ///   with other transactions.
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn delete_keys(&self, stash: &mut Stash, keys: BTreeSet<K>) -> Result<(), StashError>
    where
        K: Clone + 'static,
        V: Clone,
    {
        use futures::StreamExt;

        let name = self.name.to_string();
        stash
            .with_transaction(move |tx| {
                Box::pin(async move {
                    let collection = tx.collection::<K, V>(&name).await?;
                    let lower = tx.upper(collection.id).await?;
                    let mut batch = collection.make_batch_lower(lower)?;

                    let tx = &tx;

                    let kv_results: Vec<(K, Result<Option<V>, StashError>)> =
                        futures::stream::iter(keys.into_iter())
                            .map(|key| async move {
                                (key.clone(), tx.peek_key_one(collection.clone(), &key).await)
                            })
                            .buffer_unordered(10)
                            .collect()
                            .await;

                    for (key, val) in kv_results {
                        if let Some(v) = val? {
                            collection.append_to_batch(&mut batch, &key, &v, -1);
                        }
                    }
                    tx.append(vec![batch]).await?;
                    Ok(())
                })
            })
            .await
    }
}

/// TableTransaction emulates some features of a typical SQL transaction over
/// table for a [`StashCollection`].
///
/// It supports:
/// - uniqueness constraints
/// - transactional reads and writes (including read-your-writes before commit)
///
/// `K` is the primary key type. Multiple entries with the same key are disallowed.
/// `V` is the an arbitrary value type.
///
/// To finalize, add the results of [`TableTransaction::pending()`] to an
/// [`AppendBatch`].
pub struct TableTransaction<K, V> {
    initial: BTreeMap<K, V>,
    // The desired state of keys after commit. `None` means the value will be
    // deleted.
    pending: BTreeMap<K, Option<V>>,
    uniqueness_violation: fn(a: &V, b: &V) -> bool,
}

impl<K, V> TableTransaction<K, V>
where
    K: Ord + Eq + Clone,
    V: Ord + Clone,
{
    /// Create a new TableTransaction with initial data. `uniqueness_violation` is a function
    /// whether there is a uniqueness violation among two values.
    ///
    /// Internally the [`Stash`] serializes data as protobuf. All fields in a proto message are
    /// optional, which makes using them in Rust cumbersome. Generic parameters `KP` and `VP` are
    /// protobuf types which deserialize to `K` and `V` that a [`TableTransaction`] is generic
    /// over.
    pub fn new<KP, VP>(
        initial: BTreeMap<KP, VP>,
        uniqueness_violation: fn(a: &V, b: &V) -> bool,
    ) -> Result<Self, TryFromProtoError>
    where
        K: RustType<KP>,
        V: RustType<VP>,
    {
        let initial = initial
            .into_iter()
            .map(RustType::from_proto)
            .collect::<Result<_, _>>()?;

        Ok(Self {
            initial,
            pending: BTreeMap::new(),
            uniqueness_violation,
        })
    }

    /// Consumes and returns the pending changes and their diffs. `Diff` is
    /// guaranteed to be 1 or -1.
    pub fn pending<KP, VP>(self) -> Vec<(KP, VP, Diff)>
    where
        K: RustType<KP>,
        V: RustType<VP>,
    {
        soft_assert!(self.verify().is_ok());
        // Pending describes the desired final state for some keys. K,V pairs should be
        // retracted if they already exist and were deleted or are being updated.
        self.pending
            .into_iter()
            .map(|(k, v)| match self.initial.get(&k) {
                Some(initial_v) => {
                    let mut diffs = vec![(k.clone(), initial_v.clone(), -1)];
                    if let Some(v) = v {
                        diffs.push((k, v, 1));
                    }
                    diffs
                }
                None => {
                    if let Some(v) = v {
                        vec![(k, v, 1)]
                    } else {
                        vec![]
                    }
                }
            })
            .flatten()
            .map(|(key, val, diff)| (key.into_proto(), val.into_proto(), diff))
            .collect()
    }

    fn verify(&self) -> Result<(), StashError> {
        // Compare each value to each other value and ensure they are unique.
        let items = self.items();
        for (i, vi) in items.values().enumerate() {
            for (j, vj) in items.values().enumerate() {
                if i != j && (self.uniqueness_violation)(vi, vj) {
                    return Err("uniqueness violation".into());
                }
            }
        }
        Ok(())
    }

    /// Iterates over the items viewable in the current transaction in arbitrary
    /// order.
    pub fn for_values<F: FnMut(&K, &V)>(&self, mut f: F) {
        let mut seen = BTreeSet::new();
        for (k, v) in self.pending.iter() {
            seen.insert(k);
            // Deleted items don't exist so shouldn't be visited, but still suppress
            // visiting the key later.
            if let Some(v) = v {
                f(k, v);
            }
        }
        for (k, v) in self.initial.iter() {
            // Add on initial items that don't have updates.
            if !seen.contains(k) {
                f(k, v);
            }
        }
    }

    /// Returns the current value of `k`.
    pub fn get(&self, k: &K) -> Option<&V> {
        if let Some(v) = self.pending.get(k) {
            v.as_ref()
        } else if let Some(v) = self.initial.get(k) {
            Some(v)
        } else {
            None
        }
    }

    /// Returns the items viewable in the current transaction.
    pub fn items(&self) -> BTreeMap<K, V> {
        let mut items = BTreeMap::new();
        self.for_values(|k, v| {
            items.insert(k.clone(), v.clone());
        });
        items
    }

    /// Iterates over the items viewable in the current transaction, and provides a
    /// Vec where additional pending items can be inserted, which will be appended
    /// to current pending items. Does not verify unqiueness.
    fn for_values_mut<F: FnMut(&mut BTreeMap<K, Option<V>>, &K, &V)>(&mut self, mut f: F) {
        let mut pending = BTreeMap::new();
        self.for_values(|k, v| f(&mut pending, k, v));
        self.pending.extend(pending);
    }

    /// Inserts a new k,v pair.
    ///
    /// Returns an error if the uniqueness check failed or the key already exists.
    pub fn insert(&mut self, k: K, v: V) -> Result<(), StashError> {
        let mut violation = None;
        self.for_values(|for_k, for_v| {
            if &k == for_k {
                violation = Some("duplicate key".to_string());
            }
            if (self.uniqueness_violation)(for_v, &v) {
                violation = Some("uniqueness violation".to_string());
            }
        });
        if let Some(violation) = violation {
            return Err(violation.into());
        }
        self.pending.insert(k, Some(v));
        soft_assert!(self.verify().is_ok());
        Ok(())
    }

    /// Updates k, v pairs. `f` is a function that can return `Some(V)` if the
    /// value should be updated, otherwise `None`. Returns the number of changed
    /// entries.
    ///
    /// Returns an error if the uniqueness check failed.
    pub fn update<F: Fn(&K, &V) -> Option<V>>(&mut self, f: F) -> Result<Diff, StashError> {
        let mut changed = 0;
        // Keep a copy of pending in case of uniqueness violation.
        let pending = self.pending.clone();
        self.for_values_mut(|p, k, v| {
            if let Some(next) = f(k, v) {
                changed += 1;
                p.insert(k.clone(), Some(next));
            }
        });
        // Check for uniqueness violation.
        if let Err(err) = self.verify() {
            self.pending = pending;
            Err(err)
        } else {
            Ok(changed)
        }
    }

    /// Set the value for a key. Returns the previous entry if the key existed,
    /// otherwise None.
    ///
    /// Returns an error if the uniqueness check failed.
    pub fn set(&mut self, k: K, v: Option<V>) -> Result<Option<V>, StashError> {
        // Save the pending value for the key so we can restore it in case of
        // uniqueness violation.
        let restore = self.pending.get(&k).cloned();

        let prev = match self.pending.entry(k.clone()) {
            // key hasn't been set in this txn yet. Set it and return the
            // initial txn's value of k.
            Entry::Vacant(e) => {
                e.insert(v);
                self.initial.get(&k).cloned()
            }
            // key has been set in this txn. Set it and return the previous
            // pending value.
            Entry::Occupied(mut e) => e.insert(v),
        };

        // Check for uniqueness violation.
        if let Err(err) = self.verify() {
            // Revert self.pending to the state it was in before calling this
            // function.
            match restore {
                Some(v) => {
                    self.pending.insert(k, v);
                }
                None => {
                    self.pending.remove(&k);
                }
            }
            Err(err)
        } else {
            Ok(prev)
        }
    }

    /// Deletes items for which `f` returns true. Returns the keys and values of
    /// the deleted entries.
    pub fn delete<F: Fn(&K, &V) -> bool>(&mut self, f: F) -> Vec<(K, V)> {
        let mut deleted = Vec::new();
        self.for_values_mut(|p, k, v| {
            if f(k, v) {
                deleted.push((k.clone(), v.clone()));
                p.insert(k.clone(), None);
            }
        });
        soft_assert!(self.verify().is_ok());
        deleted
    }
}
