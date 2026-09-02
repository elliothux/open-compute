//! Reproducible P5 exact-vector benchmark over one immutable in-memory snapshot.

use open_compute_search::{DistanceMetric, ExactCandidate, ExactTopK};
use serde_json::{Value, json};
use std::hint::black_box;
use std::mem::size_of;
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

const COUNTS: [usize; 4] = [10_000, 50_000, 100_000, 250_000];
const DIMENSIONS: [usize; 4] = [384, 768, 1_024, 1_536];
const SELECTIVITIES: [usize; 3] = [1, 10, 100];
const CONCURRENCY: [usize; 3] = [1, 4, 16];
const TOP_K: usize = 10;

#[derive(Debug)]
struct Snapshot {
    dimensions: usize,
    ids: Vec<String>,
    values: Vec<f32>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let quick = std::env::var_os("OC_P5_BENCH_QUICK").is_some();
    let samples = std::env::var("OC_P5_BENCH_SAMPLES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0 && *value <= 20)
        .unwrap_or(3);
    let selected_count = selected_axis("OC_P5_BENCH_COUNT", &COUNTS)?;
    let selected_dimensions = selected_axis("OC_P5_BENCH_DIMENSIONS", &DIMENSIONS)?;
    let counts = selected_count
        .as_ref()
        .map_or(if quick { &COUNTS[..1] } else { &COUNTS[..] }, |value| {
            std::slice::from_ref(value)
        });
    let dimensions = selected_dimensions.as_ref().map_or(
        if quick {
            &DIMENSIONS[..1]
        } else {
            &DIMENSIONS[..]
        },
        |value| std::slice::from_ref(value),
    );
    let started = Instant::now();
    let mut matrix = Vec::new();
    for &count in counts {
        for &dimension in dimensions {
            let snapshot = Arc::new(build_snapshot(count, dimension)?);
            let samples_ms = measure_samples(&snapshot, 100, samples)?;
            matrix.push(json!({
                "vectors": count,
                "dimensions": dimension,
                "snapshot_bytes": snapshot_weight(&snapshot),
                "samples_ms": samples_ms,
                "p50_ms": percentile(&samples_ms, 50),
                "p95_ms": percentile(&samples_ms, 95),
            }));
        }
    }

    let profile_count = if quick { 10_000 } else { 100_000 };
    let profile_dimensions = if quick { 384 } else { 768 };
    let profile = Arc::new(build_snapshot(profile_count, profile_dimensions)?);
    let mut selectivity = Vec::new();
    for percent in SELECTIVITIES {
        let samples_ms = measure_samples(&profile, percent, samples)?;
        selectivity.push(json!({
            "percent": percent,
            "scored_candidates": profile_count.saturating_mul(percent) / 100,
            "samples_ms": samples_ms,
            "p95_ms": percentile(&samples_ms, 95),
        }));
    }

    let mut concurrency = Vec::new();
    for workers in CONCURRENCY {
        let (elapsed, worker_ms) = concurrent_scan(&profile, workers)?;
        concurrency.push(json!({
            "workers": workers,
            "wall_ms": millis(elapsed),
            "worker_ms": worker_ms,
        }));
    }

    let output: Value = json!({
        "schema_version": 1,
        "mode": if quick { "quick" } else { "full" },
        "target": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "parallelism": std::thread::available_parallelism().map_or(1, usize::from),
        },
        "metric": "cosine",
        "top_k": TOP_K,
        "samples_per_case": samples,
        "matrix": matrix,
        "filter_selectivity": selectivity,
        "concurrency": concurrency,
        "elapsed_ms": millis(started.elapsed()),
    });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn build_snapshot(count: usize, dimensions: usize) -> Result<Snapshot, &'static str> {
    let component_count = count
        .checked_mul(dimensions)
        .ok_or("snapshot component count overflow")?;
    let mut values = Vec::new();
    values
        .try_reserve_exact(component_count)
        .map_err(|_| "snapshot allocation failed")?;
    let base = (0..dimensions)
        .map(|index| ((index % 257) as f32 - 128.0) / 128.0)
        .collect::<Vec<_>>();
    let mut ids = Vec::new();
    ids.try_reserve_exact(count)
        .map_err(|_| "identifier allocation failed")?;
    for row in 0..count {
        values.extend_from_slice(&base);
        if let Some(first) = values.get_mut(row.saturating_mul(dimensions)) {
            *first = ((row % 509) as f32 - 254.0) / 254.0;
        }
        ids.push(format!("v{row:08}"));
    }
    Ok(Snapshot {
        dimensions,
        ids,
        values,
    })
}

fn selected_axis(
    variable: &str,
    allowed: &[usize],
) -> Result<Option<usize>, Box<dyn std::error::Error>> {
    let Some(raw) = std::env::var_os(variable) else {
        return Ok(None);
    };
    let value = raw
        .to_str()
        .ok_or("benchmark selector is not UTF-8")?
        .parse::<usize>()?;
    if !allowed.contains(&value) {
        return Err(format!("{variable} does not name a matrix value").into());
    }
    Ok(Some(value))
}

fn measure_samples(
    snapshot: &Snapshot,
    selectivity_percent: usize,
    samples: usize,
) -> Result<Vec<u128>, Box<dyn std::error::Error>> {
    let _ = scan(snapshot, selectivity_percent)?;
    let mut elapsed = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        black_box(scan(snapshot, selectivity_percent)?);
        elapsed.push(millis(started.elapsed()));
    }
    Ok(elapsed)
}

fn scan(
    snapshot: &Snapshot,
    selectivity_percent: usize,
) -> Result<f64, Box<dyn std::error::Error>> {
    let query = snapshot
        .values
        .get(..snapshot.dimensions)
        .ok_or("empty benchmark snapshot")?;
    let mut top = ExactTopK::new(DistanceMetric::Cosine, query, TOP_K)?;
    for row in 0..snapshot.ids.len() {
        if row % 100 >= selectivity_percent {
            continue;
        }
        let start = row
            .checked_mul(snapshot.dimensions)
            .ok_or("candidate offset overflow")?;
        let end = start
            .checked_add(snapshot.dimensions)
            .ok_or("candidate offset overflow")?;
        top.push(ExactCandidate {
            id: &snapshot.ids[row],
            values: snapshot
                .values
                .get(start..end)
                .ok_or("candidate range is corrupt")?,
        })?;
    }
    Ok(top.finish().first().map_or(0.0, |item| item.score))
}

fn concurrent_scan(
    snapshot: &Arc<Snapshot>,
    workers: usize,
) -> Result<(Duration, Vec<u128>), Box<dyn std::error::Error>> {
    let barrier = Arc::new(Barrier::new(workers));
    let started = Instant::now();
    let handles = (0..workers)
        .map(|_| {
            let snapshot = snapshot.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let worker_started = Instant::now();
                scan(&snapshot, 100)
                    .map(|score| (millis(worker_started.elapsed()), score))
                    .map_err(|error| error.to_string())
            })
        })
        .collect::<Vec<_>>();
    let mut worker_ms = Vec::with_capacity(workers);
    for handle in handles {
        let (elapsed, score) = handle.join().map_err(|_| "benchmark worker panicked")??;
        black_box(score);
        worker_ms.push(elapsed);
    }
    worker_ms.sort_unstable();
    Ok((started.elapsed(), worker_ms))
}

fn percentile(samples: &[u128], percentile: usize) -> u128 {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let index = sorted
        .len()
        .saturating_mul(percentile)
        .div_ceil(100)
        .saturating_sub(1)
        .min(sorted.len().saturating_sub(1));
    sorted.get(index).copied().unwrap_or(0)
}

fn snapshot_weight(snapshot: &Snapshot) -> usize {
    snapshot
        .values
        .len()
        .saturating_mul(size_of::<f32>())
        .saturating_add(snapshot.ids.iter().map(String::len).sum::<usize>())
}

fn millis(duration: Duration) -> u128 {
    duration.as_micros().div_ceil(1_000)
}
