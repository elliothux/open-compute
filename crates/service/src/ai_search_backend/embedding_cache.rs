//! Per-request query-embedding deduplication across multi-instance retrieval.

use super::*;
use std::future::Future;

pub(super) type SharedQueryEmbeddings =
    Arc<tokio::sync::Mutex<HashMap<(String, String), Arc<tokio::sync::OnceCell<Vec<f32>>>>>>;

pub(super) fn new_query_embedding_cache() -> SharedQueryEmbeddings {
    Arc::new(tokio::sync::Mutex::new(HashMap::new()))
}

pub(super) async fn cached_query_embedding<F, Fut>(
    cache: &SharedQueryEmbeddings,
    key: (String, String),
    load: F,
) -> Result<Vec<f32>, PlatformError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Vec<f32>, PlatformError>>,
{
    let cell = {
        let mut entries = cache.lock().await;
        entries
            .entry(key)
            .or_insert_with(|| Arc::new(tokio::sync::OnceCell::new()))
            .clone()
    };
    cell.get_or_try_init(load).await.cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn identical_contract_and_query_are_loaded_once() {
        let cache = new_query_embedding_cache();
        let calls = Arc::new(AtomicUsize::new(0));
        let first_calls = calls.clone();
        let second_calls = calls.clone();
        let first =
            cached_query_embedding(&cache, ("contract".into(), "query".into()), || async move {
                first_calls.fetch_add(1, Ordering::SeqCst);
                tokio::task::yield_now().await;
                Ok(vec![1.0])
            });
        let second =
            cached_query_embedding(&cache, ("contract".into(), "query".into()), || async move {
                second_calls.fetch_add(1, Ordering::SeqCst);
                Ok(vec![2.0])
            });
        let (first, second) = tokio::join!(first, second);
        assert_eq!(first.unwrap(), vec![1.0]);
        assert_eq!(second.unwrap(), vec![1.0]);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
