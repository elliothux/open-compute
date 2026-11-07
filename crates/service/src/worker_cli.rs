//! Offline developer-tool input bridge to the canonical Worker bundle encoder.

use base64::Engine as _;
use open_compute_core::{ErrorCode, PlatformError};
use open_compute_workers::bundle::WORKER_BUNDLE_SCHEMA_VERSION;
use open_compute_workers::{BundleLimits, CanonicalBundle, ModuleInput, ModuleType};
use serde::Deserialize;
use std::io::{Read, Write};

// Bound base64 payload plus JSON framing, including six-byte escaped name characters.
const MAX_INPUT_BYTES: u64 = (BundleLimits::DEFAULT.max_total_module_bytes.div_ceil(3) * 4
    + BundleLimits::DEFAULT.max_manifest_bytes * 6) as u64;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BundleInput {
    schema_version: u32,
    main_module: String,
    modules: Vec<EncodedModule>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EncodedModule {
    name: String,
    #[serde(rename = "type")]
    module_type: ModuleType,
    bytes_base64: String,
}

pub(crate) fn encode_bundle(
    input: impl Read,
    output: &mut impl Write,
) -> Result<(), PlatformError> {
    let mut bytes = Vec::new();
    input
        .take(MAX_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid("failed to read Worker build input"))?;
    if bytes.len() as u64 > MAX_INPUT_BYTES {
        return Err(too_large());
    }
    let input: BundleInput = serde_json::from_slice(&bytes)
        .map_err(|_| invalid("Worker build input JSON is invalid"))?;
    if input.schema_version != WORKER_BUNDLE_SCHEMA_VERSION {
        return Err(invalid("Worker build input schema is unsupported"));
    }
    let limits = BundleLimits::default();
    if input.modules.len() > limits.max_modules {
        return Err(too_large());
    }
    let mut modules = Vec::with_capacity(input.modules.len());
    let mut total = 0_usize;
    for module in input.modules {
        if module.bytes_base64.len() > limits.max_total_module_bytes.div_ceil(3) * 4 {
            return Err(too_large());
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(module.bytes_base64)
            .map_err(|_| invalid("Worker module base64 is invalid"))?;
        total = total.checked_add(bytes.len()).ok_or_else(too_large)?;
        if total > limits.max_total_module_bytes {
            return Err(too_large());
        }
        modules.push(ModuleInput {
            name: module.name,
            module_type: module.module_type,
            bytes,
        });
    }
    let bundle = CanonicalBundle::build(&input.main_module, modules, limits)?;
    output
        .write_all(bundle.bytes())
        .map_err(|_| invalid("failed to write Worker bundle"))
}

fn invalid(message: &'static str) -> PlatformError {
    PlatformError::new(ErrorCode::BundleInvalid, message)
}

fn too_large() -> PlatformError {
    PlatformError::new(
        ErrorCode::BundleTooLarge,
        "Worker build input exceeds bundle limits",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(source: &[u8]) -> serde_json::Value {
        serde_json::json!({
            "schemaVersion": 1,
            "mainModule": "worker.js",
            "modules": [{"name": "worker.js", "type": "esModule",
                "bytesBase64": base64::engine::general_purpose::STANDARD.encode(source)}],
        })
    }

    #[test]
    fn output_is_the_existing_canonical_bundle_format() {
        let source = b"export default {fetch() {return new Response('ok')}}";
        let mut output = Vec::new();
        encode_bundle(input(source).to_string().as_bytes(), &mut output).unwrap();
        let expected = CanonicalBundle::build(
            "worker.js",
            vec![ModuleInput {
                name: "worker.js".to_owned(),
                module_type: ModuleType::EsModule,
                bytes: source.to_vec(),
            }],
            BundleLimits::default(),
        )
        .unwrap();
        assert_eq!(output, expected.bytes());
        assert_eq!(
            CanonicalBundle::parse(output, BundleLimits::default()).unwrap(),
            expected
        );
    }

    #[test]
    fn full_module_budget_round_trips_through_bounded_base64_input() {
        let limits = BundleLimits::default();
        let main_bytes = 9 * 1024 * 1024;
        let mut modules = vec![serde_json::json!({"name": "index.js", "type": "esModule",
                "bytesBase64": base64::engine::general_purpose::STANDARD
                    .encode(vec![b' '; main_bytes])})];
        modules.push(serde_json::json!({"name": "data.bin", "type": "data",
            "bytesBase64": base64::engine::general_purpose::STANDARD
                .encode(vec![0; limits.max_total_module_bytes - main_bytes])}));
        for index in 2..132 {
            modules.push(serde_json::json!({"name": format!("data{index}.bin"),
                "type": "data", "bytesBase64": ""}));
        }
        let bytes = serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 1, "mainModule": "index.js", "modules": modules,
        }))
        .unwrap();
        assert!(bytes.len() > 24 * 1024 * 1024);
        let mut output = Vec::new();
        encode_bundle(bytes.as_slice(), &mut output).unwrap();
        let bundle = CanonicalBundle::parse(output, limits).unwrap();
        assert_eq!(bundle.manifest().modules.len(), 132);
        let main = bundle
            .manifest()
            .modules
            .iter()
            .find(|module| module.name == "index.js");
        assert_eq!(main.unwrap().size, main_bytes as u64);
        assert_eq!(
            bundle
                .manifest()
                .modules
                .iter()
                .map(|module| module.size)
                .sum::<u64>(),
            limits.max_total_module_bytes as u64
        );
    }

    #[test]
    fn invalid_inputs_write_no_artifact_and_do_not_echo_source() {
        let base = input(b"private-module-source");
        let mut unsupported = base.clone();
        unsupported["schemaVersion"] = 2.into();
        let mut unknown = base.clone();
        unknown["secret"] = "private-module-source".into();
        let mut bad_base64 = base.clone();
        bad_base64["modules"][0]["bytesBase64"] = "private-module-source".into();
        let mut traversal = base.clone();
        traversal["modules"][0]["name"] = "../private-module-source".into();
        for value in [
            unsupported,
            unknown,
            bad_base64,
            traversal,
            serde_json::json!({}),
        ] {
            let mut output = Vec::new();
            let error = encode_bundle(value.to_string().as_bytes(), &mut output).unwrap_err();
            assert_eq!(error.code(), ErrorCode::BundleInvalid);
            assert!(output.is_empty());
            assert!(!error.to_string().contains("private-module-source"));
        }
    }

    #[test]
    fn input_and_module_budgets_are_enforced_before_output() {
        let mut output = Vec::new();
        let error = encode_bundle(std::io::repeat(b' ').take(MAX_INPUT_BYTES + 2), &mut output)
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::BundleTooLarge);
        let source = vec![0; BundleLimits::default().max_total_module_bytes + 1];
        let error = encode_bundle(input(&source).to_string().as_bytes(), &mut output).unwrap_err();
        assert_eq!(error.code(), ErrorCode::BundleTooLarge);
        assert!(output.is_empty());
    }
}
