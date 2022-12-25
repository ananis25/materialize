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
// Have complaints about the noise? See the note in misc/python/cli/gen-lints.py first.
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
#![warn(clippy::collapsible_if)]
#![warn(clippy::collapsible_else_if)]
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
#![warn(clippy::from_over_into)]
// END LINT CONFIG

use std::iter::{repeat, repeat_with};
use std::str::FromStr;

use criterion::{criterion_group, criterion_main, Criterion};
use once_cell::sync::Lazy;
use timely::progress::Antichain;
use tokio::runtime::Runtime;

use mz_ore::metrics::MetricsRegistry;
use mz_stash::{Append, Postgres, PostgresFactory, Stash, StashError};

pub static FACTORY: Lazy<PostgresFactory> =
    Lazy::new(|| PostgresFactory::new(&MetricsRegistry::new()));

fn init_bench() -> (Runtime, Postgres) {
    let runtime = Runtime::new().unwrap();
    let connstr = std::env::var("POSTGRES_URL").unwrap();
    let tls = mz_postgres_util::make_tls(
        &tokio_postgres::config::Config::from_str(&connstr)
            .expect("invalid postgres url for storage stash"),
    )
    .unwrap();
    runtime
        .block_on(Postgres::clear(&connstr, tls.clone()))
        .unwrap();
    let stash = runtime
        .block_on((*FACTORY).open(connstr, None, tls))
        .unwrap();
    (runtime, stash)
}

fn bench_update(c: &mut Criterion) {
    c.bench_function("update", |b| {
        b.iter(|| {
            let (runtime, mut stash) = init_bench();

            let orders = runtime
                .block_on(stash.collection::<String, String>("orders"))
                .unwrap();
            let mut ts = 1;
            runtime.block_on(async {
                let data = ("widgets1".into(), "1".into());
                stash.update(orders, data, ts, 1).await.unwrap();
                ts += 1;
            })
        })
    });
}

fn bench_update_many(c: &mut Criterion) {
    c.bench_function("update_many", |b| {
        let (runtime, mut stash) = init_bench();

        let orders = runtime
            .block_on(stash.collection::<String, String>("orders"))
            .unwrap();
        let mut ts = 1;
        b.iter(|| {
            runtime.block_on(async {
                let data = ("widgets2".into(), "1".into());
                stash
                    .update_many(orders, repeat((data, ts, 1)).take(10))
                    .await
                    .unwrap();
                ts += 1;
            })
        })
    });
}

fn bench_consolidation(c: &mut Criterion) {
    c.bench_function("consolidation", |b| {
        let (runtime, mut stash) = init_bench();

        let orders = runtime
            .block_on(stash.collection::<String, String>("orders"))
            .unwrap();
        let mut ts = 1;
        b.iter(|| {
            runtime.block_on(async {
                let data = ("widgets3".into(), "1".into());
                stash.update(orders, data.clone(), ts, 1).await.unwrap();
                stash.update(orders, data, ts + 1, -1).await.unwrap();
                let frontier = Antichain::from_elem(ts + 2);
                stash.seal(orders, frontier.borrow()).await.unwrap();
                stash.compact(orders, frontier.borrow()).await.unwrap();
                stash.consolidate(orders.id).await.unwrap();
                ts += 2;
            })
        })
    });
}

fn bench_consolidation_large(c: &mut Criterion) {
    c.bench_function("consolidation large", |b| {
        let (runtime, mut stash) = init_bench();

        let mut ts = 0;
        let (orders, kv) = runtime.block_on(async {
            let orders = stash.collection::<String, String>("orders").await.unwrap();

            // Prepopulate the database with 100k records
            let kv = ("widgets4".into(), "1".into());
            stash
                .update_many(
                    orders,
                    repeat_with(|| {
                        let update = (kv.clone(), ts, 1);
                        ts += 1;
                        update
                    })
                    .take(100_000),
                )
                .await
                .unwrap();
            let frontier = Antichain::from_elem(ts);
            stash.seal(orders, frontier.borrow()).await.unwrap();
            (orders, kv)
        });

        let mut compact_ts = 0;
        b.iter(|| {
            runtime.block_on(async {
                ts += 1;
                // add 10k records
                stash
                    .update_many(
                        orders,
                        repeat_with(|| {
                            let update = (kv.clone(), ts, 1);
                            ts += 1;
                            update
                        })
                        .take(10_000),
                    )
                    .await
                    .unwrap();
                let frontier = Antichain::from_elem(ts);
                stash.seal(orders, frontier.borrow()).await.unwrap();
                // compact + consolidate
                compact_ts += 10_000;
                let compact_frontier = Antichain::from_elem(compact_ts);
                stash
                    .compact(orders, compact_frontier.borrow())
                    .await
                    .unwrap();
                stash.consolidate(orders.id).await.unwrap();
            })
        })
    });
}

fn bench_append(c: &mut Criterion) {
    c.bench_function("append", |b| {
        let (runtime, mut stash) = init_bench();
        const MAX: i64 = 1000;

        let orders = runtime
            .block_on(async {
                let orders = stash.collection::<String, String>("orders").await?;
                let mut batch = orders.make_batch(&mut stash).await?;
                // Skip 0 so it can be added initially.
                for i in 1..MAX {
                    orders.append_to_batch(&mut batch, &i.to_string(), &format!("_{i}"), 1);
                }
                stash.append(&[batch]).await?;
                Result::<_, StashError>::Ok(orders)
            })
            .unwrap();
        let mut i = 0;
        b.iter(|| {
            runtime.block_on(async {
                let mut batch = orders.make_batch(&mut stash).await.unwrap();
                let j = i % MAX;
                let k = (i + 1) % MAX;
                // Add the current i which doesn't exist, delete the next i
                // which is known to exist.
                orders.append_to_batch(&mut batch, &j.to_string(), &format!("_{j}"), 1);
                orders.append_to_batch(&mut batch, &k.to_string(), &format!("_{k}"), -1);
                stash.append(&[batch]).await.unwrap();
                i += 1;
            })
        })
    });
}

criterion_group!(
    benches,
    bench_append,
    bench_update,
    bench_update_many,
    bench_consolidation,
    bench_consolidation_large
);
criterion_main!(benches);
