//! Read-only UI cache. Never used for signing, sending, or live balance checks.
use anyhow::Result;
use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

type Entry<T> = Arc<tokio::sync::Mutex<Option<(Instant, Result<T, String>)>>>;
pub(crate) struct PreviewCache<T> {
    entries: Mutex<HashMap<String, Entry<T>>>,
}
impl<T> Default for PreviewCache<T> {
    fn default() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }
}
impl<T: Clone> PreviewCache<T> {
    pub(crate) async fn get<F, Fut>(&self, key: String, ttl: Duration, fetch: F) -> Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let entry = {
            let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            entries
                .entry(key)
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(None)))
                .clone()
        };
        let mut value = entry.lock().await;
        if let Some((at, result)) = value.as_ref() {
            if at.elapsed() < ttl {
                return result.clone().map_err(anyhow::Error::msg);
            }
        }
        let result = fetch().await.map_err(|e| e.to_string());
        *value = Some((Instant::now(), result.clone()));
        result.map_err(anyhow::Error::msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[tokio::test]
    async fn coalesces_requests_and_isolates_keys() {
        let cache = PreviewCache::default();
        let count = AtomicUsize::new(0);
        let read = || async {
            count.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
            Ok(7)
        };
        let (a, b) = tokio::join!(
            cache.get("a".into(), Duration::from_secs(15), read),
            cache.get("a".into(), Duration::from_secs(15), read)
        );
        assert_eq!((a.unwrap(), b.unwrap()), (7, 7));
        assert_eq!(count.load(Ordering::SeqCst), 1);
        cache
            .get("b".into(), Duration::from_secs(15), read)
            .await
            .unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 2);
        cache.get("a".into(), Duration::ZERO, read).await.unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }
    #[tokio::test]
    async fn failures_are_throttled_not_retried_by_every_window() {
        let cache = PreviewCache::<u64>::default();
        assert!(
            cache
                .get("a".into(), Duration::from_secs(15), || async {
                    anyhow::bail!("offline")
                })
                .await
                .is_err()
        );
        assert!(
            cache
                .get("a".into(), Duration::from_secs(15), || async {
                    panic!("must not retry");
                })
                .await
                .is_err()
        );
    }
}
