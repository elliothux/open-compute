//! Deterministic, bounded P1 authority-parser fuzz rehearsal.

use open_compute_core::{
    AccountId, DeploymentId, PlatformReleaseMetadataV1, PlatformSnapshotManifestV1, ResourceId,
    WorkerId,
};
use open_compute_workers::{BindingDescriptorV1, BundleLimits, CanonicalBundle, parse_loader_key};
use std::str::FromStr;
use std::time::{Duration, Instant};

const MAX_INPUT_BYTES: usize = 64 * 1024;
const DEFAULT_SECONDS: u64 = 60;
const SEED: u64 = 0x6f70_656e_636f_6d70;

fn main() {
    let seconds = parse_seconds();
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let corpus = corpus();
    let mut random = XorShift64(SEED);
    let mut cases = 0_u64;
    while Instant::now() < deadline {
        let seed = corpus[cases as usize % corpus.len()];
        let input = mutate(seed, &mut random);
        exercise(&input);
        cases = cases.saturating_add(1);
    }
    println!(
        "{{\"schema_version\":1,\"seed\":\"{SEED:016x}\",\"seconds\":{seconds},\"cases\":{cases},\"max_input_bytes\":{MAX_INPUT_BYTES},\"verdict\":\"pass\"}}"
    );
}

fn parse_seconds() -> u64 {
    let mut arguments = std::env::args().skip(1);
    match (
        arguments.next().as_deref(),
        arguments.next(),
        arguments.next(),
    ) {
        (None, None, None) => DEFAULT_SECONDS,
        (Some("--seconds"), Some(value), None) => value
            .parse::<u64>()
            .ok()
            .filter(|seconds| (1..=3_600).contains(seconds))
            .unwrap_or_else(|| usage()),
        _ => usage(),
    }
}

fn usage() -> ! {
    eprintln!("usage: p1-fuzz [--seconds 1..3600]");
    std::process::exit(2);
}

fn corpus() -> Vec<&'static [u8]> {
    vec![
        include_bytes!("../corpus/empty"),
        include_bytes!("../corpus/snapshot.json"),
        include_bytes!("../corpus/release.json"),
        include_bytes!("../corpus/binding.json"),
        include_bytes!("../corpus/path-ids.txt"),
    ]
}

fn mutate(seed: &[u8], random: &mut XorShift64) -> Vec<u8> {
    let target = if seed.is_empty() {
        (random.next() as usize % 256).saturating_add(1)
    } else {
        seed.len().min(MAX_INPUT_BYTES)
    };
    let mut output = if seed.is_empty() {
        vec![0; target]
    } else {
        seed[..target].to_vec()
    };
    let edits = 1 + random.next() as usize % 32;
    for _ in 0..edits {
        if output.is_empty() {
            output.push(random.next() as u8);
            continue;
        }
        let index = random.next() as usize % output.len();
        output[index] ^= (random.next() as u8).max(1);
        if output.len() < MAX_INPUT_BYTES && random.next().is_multiple_of(17) {
            output.insert(index, random.next() as u8);
        }
        if output.len() > 1 && random.next().is_multiple_of(19) {
            output.remove(index.min(output.len() - 1));
        }
    }
    output
}

fn exercise(input: &[u8]) {
    if let Ok(bundle) = CanonicalBundle::parse(input.to_vec(), BundleLimits::default()) {
        let canonical = bundle.bytes().to_vec();
        let reparsed = CanonicalBundle::parse(canonical.clone(), BundleLimits::default())
            .expect("accepted bundle must remain parseable");
        assert_eq!(reparsed.bytes(), canonical);
    }
    if let Ok(descriptor) = serde_json::from_slice::<BindingDescriptorV1>(input)
        && let Ok(canonical) = descriptor.canonical_bytes()
    {
        let reparsed: BindingDescriptorV1 =
            serde_json::from_slice(&canonical).expect("canonical binding descriptor");
        assert_eq!(reparsed, descriptor);
    }
    if let Ok(manifest) = serde_json::from_slice::<PlatformSnapshotManifestV1>(input)
        && manifest
            .validate(4_096, MAX_INPUT_BYTES as u64, MAX_INPUT_BYTES as u64)
            .is_ok()
    {
        assert!(manifest.canonical_unsigned_bytes().is_ok());
    }
    if let Ok(release) = serde_json::from_slice::<PlatformReleaseMetadataV1>(input)
        && release.validate()
    {
        let canonical = serde_json::to_vec(&release).expect("canonical release metadata");
        let reparsed: PlatformReleaseMetadataV1 =
            serde_json::from_slice(&canonical).expect("release round trip");
        assert_eq!(reparsed, release);
    }
    if let Ok(value) = std::str::from_utf8(input) {
        canonical_id::<AccountId>(value);
        canonical_id::<WorkerId>(value);
        canonical_id::<DeploymentId>(value);
        canonical_id::<ResourceId>(value);
        if let Ok((account, worker, deployment)) = parse_loader_key(value) {
            assert_eq!(
                value,
                format!("{account}/{worker}/{deployment}"),
                "accepted loader key must be canonical"
            );
        }
    }
}

fn canonical_id<T>(value: &str)
where
    T: FromStr + ToString,
{
    if let Ok(id) = T::from_str(value) {
        assert_eq!(
            id.to_string(),
            value,
            "accepted identifier must be canonical"
        );
    }
}

struct XorShift64(u64);

impl XorShift64 {
    fn next(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }
}
