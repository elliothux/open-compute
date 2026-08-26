//! Generation-safe scheduler mutation notification.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

/// Process-local notification emitted after a scheduler projection mutation commits.
#[derive(Debug, Default)]
pub struct SchedulerWakeSignal {
    generation: AtomicU64,
    waiters: Mutex<WakeWaiters>,
}

#[derive(Debug, Default)]
struct WakeWaiters {
    next_id: u64,
    values: BTreeMap<u64, Waker>,
}

impl SchedulerWakeSignal {
    /// Current monotonically increasing mutation generation.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Publish one committed mutation and wake every current waiter.
    pub fn notify(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        let wakers = {
            let mut waiters = self
                .waiters
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::take(&mut waiters.values)
        };
        for (_, waker) in wakers {
            waker.wake();
        }
    }

    /// Wait until the generation differs from the caller's observation.
    #[must_use]
    pub fn notified_since(self: &Arc<Self>, observed: u64) -> SchedulerWakeFuture {
        SchedulerWakeFuture {
            signal: self.clone(),
            observed,
            waiter_id: None,
        }
    }
}

/// Future completed by a committed scheduler mutation after an observed generation.
#[derive(Debug)]
pub struct SchedulerWakeFuture {
    signal: Arc<SchedulerWakeSignal>,
    observed: u64,
    waiter_id: Option<u64>,
}

impl Future for SchedulerWakeFuture {
    type Output = u64;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let current = self.signal.generation();
        if current != self.observed {
            self.remove_waiter();
            return Poll::Ready(current);
        }
        let signal = self.signal.clone();
        let mut waiters = signal
            .waiters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = signal.generation();
        if current != self.observed {
            if let Some(id) = self.waiter_id.take() {
                waiters.values.remove(&id);
            }
            return Poll::Ready(current);
        }
        let id = self.waiter_id.unwrap_or_else(|| {
            let id = waiters.next_id;
            waiters.next_id = waiters.next_id.saturating_add(1);
            id
        });
        waiters.values.insert(id, context.waker().clone());
        drop(waiters);
        self.waiter_id = Some(id);
        Poll::Pending
    }
}

impl SchedulerWakeFuture {
    fn remove_waiter(&mut self) {
        let Some(id) = self.waiter_id.take() else {
            return;
        };
        self.signal
            .waiters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values
            .remove(&id);
    }
}

impl Drop for SchedulerWakeFuture {
    fn drop(&mut self) {
        self.remove_waiter();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn notification_before_and_after_wait_registration_is_not_lost() {
        let signal = Arc::new(SchedulerWakeSignal::default());
        let before = signal.generation();
        signal.notify();
        assert_eq!(signal.notified_since(before).await, before + 1);

        let observed = signal.generation();
        let mut waiter = signal.notified_since(observed);
        assert!(futures::poll!(&mut waiter).is_pending());
        signal.notify();
        assert_eq!(waiter.await, observed + 1);
    }
}
