//! Strict request payloads shared by AI Search retrieval and chat routes.

use super::*;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SearchPayload {
    pub(super) query: Option<String>,
    pub(super) messages: Option<Vec<WireMessage>>,
    #[serde(default)]
    pub(super) ai_search_options: SearchOptions,
}

impl SearchPayload {
    pub(super) fn query_text(&self) -> Result<String, PlatformError> {
        match (&self.query, &self.messages) {
            (Some(query), None) if !query.is_empty() => Ok(query.clone()),
            (None, Some(messages)) => messages
                .iter()
                .rev()
                .find(|message| message.role == "user")
                .and_then(|message| message.content.clone())
                .filter(|content| !content.is_empty())
                .ok_or_else(protocol),
            _ => Err(protocol()),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SearchOptions {
    pub(super) retrieval: Option<RetrievalOptions>,
    pub(super) query_rewrite: Option<ModelToggle>,
    pub(super) reranking: Option<RerankOptions>,
    pub(super) instance_ids: Option<Vec<String>>,
}

impl SearchOptions {
    pub(super) fn return_on_failure(&self) -> bool {
        self.retrieval
            .as_ref()
            .and_then(|options| options.return_on_failure)
            .unwrap_or(true)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RetrievalOptions {
    pub(super) retrieval_type: Option<String>,
    pub(super) fusion_method: Option<String>,
    pub(super) keyword_match_mode: Option<String>,
    pub(super) match_threshold: Option<f64>,
    pub(super) max_num_results: Option<u8>,
    pub(super) filters: Option<Value>,
    pub(super) context_expansion: Option<u8>,
    pub(super) metadata_only: Option<bool>,
    pub(super) return_on_failure: Option<bool>,
    pub(super) boost_by: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ModelToggle {
    pub(super) enabled: Option<bool>,
    pub(super) model: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RerankOptions {
    pub(super) enabled: Option<bool>,
    pub(super) model: Option<String>,
    pub(super) match_threshold: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChatPayload {
    pub(super) messages: Vec<WireMessage>,
    pub(super) model: Option<String>,
    pub(super) stream: Option<bool>,
    #[serde(default)]
    pub(super) ai_search_options: SearchOptions,
}

impl ChatPayload {
    pub(super) fn as_search(&self) -> SearchPayload {
        SearchPayload {
            query: None,
            messages: Some(self.messages.clone()),
            ai_search_options: self.ai_search_options.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WireMessage {
    pub(super) role: String,
    pub(super) content: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_instance_failure_default_is_true_and_can_be_disabled() {
        assert!(SearchOptions::default().return_on_failure());
        let options: SearchOptions = serde_json::from_value(json!({
            "retrieval": {"return_on_failure": false}
        }))
        .unwrap();
        assert!(!options.return_on_failure());
    }
}
