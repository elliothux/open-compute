use super::AiSearchInstanceStorageContract;
use open_compute_core::{
    AiEmbeddingMetric, ResolvedEmbeddingModelContract, ResolvedTokenizerContract,
};
use sha2::{Digest as _, Sha256};

pub(super) fn valid_instance_contract(contract: &AiSearchInstanceStorageContract<'_>) -> bool {
    if contract.public_config_json.len() > 65_536 || contract.model_contract_json.len() > 65_536 {
        return false;
    }
    let Ok(public) = serde_json::from_slice::<serde_json::Value>(contract.public_config_json)
    else {
        return false;
    };
    let Ok(model) = serde_json::from_slice::<serde_json::Value>(contract.model_contract_json)
    else {
        return false;
    };
    let Some(public) = public.as_object() else {
        return false;
    };
    let index = public
        .get("index_method")
        .and_then(serde_json::Value::as_object);
    let vector = index
        .and_then(|index| index.get("vector"))
        .and_then(serde_json::Value::as_bool);
    let keyword = index
        .and_then(|index| index.get("keyword"))
        .and_then(serde_json::Value::as_bool);
    let valid_public = vector == Some(contract.vector_enabled)
        && keyword == Some(contract.keyword_enabled)
        && public
            .get("chunk")
            .is_some_and(serde_json::Value::is_boolean)
        && public
            .get("chunk_size")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|value| value > 0)
        && public
            .get("chunk_overlap")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|value| value <= 30)
        && public
            .get("score_threshold")
            .and_then(serde_json::Value::as_f64)
            .is_some_and(|value| value.is_finite() && (0.0..=1.0).contains(&value))
        && public
            .get("max_num_results")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|value| (1..=50).contains(&value))
        && public
            .get("fusion_method")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| matches!(value, "max" | "rrf"))
        && public
            .get("custom_metadata")
            .is_some_and(serde_json::Value::is_array)
        && public
            .get("metadata")
            .is_some_and(serde_json::Value::is_object);
    if !valid_public {
        return false;
    }
    if contract.vector_enabled {
        serde_json::from_value::<ResolvedEmbeddingModelContract>(model)
            .is_ok_and(|model| valid_embedding_contract(&model, contract.dimensions))
    } else {
        let Some(model) = model.as_object() else {
            return false;
        };
        model.get("kind").and_then(serde_json::Value::as_str) == Some("keyword_only")
            && model
                .get("schemaVersion")
                .and_then(serde_json::Value::as_u64)
                == Some(1)
            && model
                .get("tokenizerContract")
                .and_then(serde_json::Value::as_object)
                .cloned()
                .and_then(|tokenizer| {
                    serde_json::from_value::<ResolvedTokenizerContract>(serde_json::Value::Object(
                        tokenizer,
                    ))
                    .ok()
                })
                .is_some_and(|tokenizer| valid_tokenizer_contract(&tokenizer))
    }
}

fn valid_embedding_contract(model: &ResolvedEmbeddingModelContract, dimensions: u32) -> bool {
    model.dimensions == dimensions
        && model.metric == AiEmbeddingMetric::Cosine
        && model.max_input_tokens > 0
        && nonempty(&model.embedding_alias)
        && nonempty(&model.backend_name)
        && valid_sha256(&model.backend_contract_sha256)
        && model.protocol == "openai_embeddings_v1"
        && valid_sha256(&model.endpoint_sha256)
        && valid_auth_shape(model)
        && valid_sha256(&model.headers_sha256)
        && nonempty(&model.remote_model)
        && model.provider_revision.as_deref().is_none_or(nonempty)
        && nonempty(&model.profile)
        && valid_sha256(&model.profile_contract_sha256)
        && nonempty(&model.tokenizer_revision)
        && valid_sha256(&model.tokenizer_artifact_sha256)
        && valid_embedded_contract_digest(model)
}

fn valid_tokenizer_contract(contract: &ResolvedTokenizerContract) -> bool {
    contract.max_input_tokens > 0
        && nonempty(&contract.embedding_alias)
        && nonempty(&contract.profile)
        && valid_sha256(&contract.profile_contract_sha256)
        && nonempty(&contract.tokenizer_revision)
        && valid_sha256(&contract.tokenizer_artifact_sha256)
        && valid_embedded_contract_digest(contract)
}

fn valid_auth_shape(model: &ResolvedEmbeddingModelContract) -> bool {
    match model.auth_kind.as_str() {
        "bearer" | "none" => model.auth_header_name.is_none(),
        "header" => model.auth_header_name.as_deref().is_some_and(nonempty),
        _ => false,
    }
}

fn valid_embedded_contract_digest<T>(contract: &T) -> bool
where
    T: serde::Serialize + Clone + EmbeddedContractDigest,
{
    let claimed = contract.contract_sha256();
    if !valid_sha256(claimed) {
        return false;
    }
    let mut unsigned = contract.clone();
    unsigned.clear_contract_sha256();
    serde_json::to_vec(&unsigned).is_ok_and(|bytes| hex::encode(Sha256::digest(bytes)) == claimed)
}

trait EmbeddedContractDigest {
    fn contract_sha256(&self) -> &str;
    fn clear_contract_sha256(&mut self);
}

impl EmbeddedContractDigest for ResolvedEmbeddingModelContract {
    fn contract_sha256(&self) -> &str {
        &self.contract_sha256
    }

    fn clear_contract_sha256(&mut self) {
        self.contract_sha256.clear();
    }
}

impl EmbeddedContractDigest for ResolvedTokenizerContract {
    fn contract_sha256(&self) -> &str {
        &self.contract_sha256
    }

    fn clear_contract_sha256(&mut self) {
        self.contract_sha256.clear();
    }
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn nonempty(value: &str) -> bool {
    !value.is_empty()
}
