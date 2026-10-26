use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::Notify;

/// The exchanges of each thread that have not yet written their first record, so records can be
/// written in the order their exchanges began.
#[derive(Default)]
pub(super) struct Queue {
    waiting: Mutex<HashMap<String, BTreeSet<u64>>>,
    changed: Notify,
}

impl Queue {
    pub(super) fn join(self: &Arc<Self>, thread: &str, exchange: u64) -> Place {
        let mut waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
        waiting.entry(thread.to_owned()).or_default().insert(exchange);
        Place {
            queue: self.clone(),
            thread: thread.to_owned(),
            exchange,
        }
    }

    fn is_clear_before(&self, thread: &str, exchange: u64) -> bool {
        let waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
        waiting
            .get(thread)
            .and_then(|exchanges| exchanges.first())
            .is_none_or(|first| *first >= exchange)
    }

    fn leave(&self, thread: &str, exchange: u64) {
        let mut waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(exchanges) = waiting.get_mut(thread) {
            exchanges.remove(&exchange);
            if exchanges.is_empty() {
                waiting.remove(thread);
            }
        }
        drop(waiting);
        self.changed.notify_waiters();
    }
}

/// An exchange's place in line among the exchanges of its thread, held until the exchange ends.
pub struct Place {
    queue: Arc<Queue>,
    thread: String,
    exchange: u64,
}

impl Place {
    /// Waits until every earlier exchange of the thread has written its first record or ended.
    pub async fn wait_for_turn(&self) {
        loop {
            let changed = self.queue.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.queue.is_clear_before(&self.thread, self.exchange) {
                return;
            }
            changed.await;
        }
    }

    /// Lets later exchanges of the thread go ahead. Ending the exchange does the same.
    pub fn leave(&self) {
        self.queue.leave(&self.thread, self.exchange);
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        self.leave();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::timeout;

    const SHORT: Duration = Duration::from_millis(50);

    #[tokio::test]
    async fn the_first_exchange_of_a_thread_does_not_wait() {
        // It goes ahead at once when no earlier exchange of its thread is in line
        let queue = Arc::<Queue>::default();
        let first = queue.join("t1", 1);
        let _second = queue.join("t1", 2);

        assert!(timeout(SHORT, first.wait_for_turn()).await.is_ok());
    }

    #[tokio::test]
    async fn a_later_exchange_waits_until_the_earlier_one_leaves() {
        // It is held while the earlier exchange is in line, and goes ahead once that one leaves
        let queue = Arc::<Queue>::default();
        let first = queue.join("t1", 1);
        let second = queue.join("t1", 2);

        assert!(timeout(SHORT, second.wait_for_turn()).await.is_err());
        first.leave();

        assert!(timeout(SHORT, second.wait_for_turn()).await.is_ok());
    }

    #[tokio::test]
    async fn a_later_exchange_goes_ahead_when_the_earlier_one_is_dropped() {
        // It does not wait for an exchange that ended without leaving
        let queue = Arc::<Queue>::default();
        let first = queue.join("t1", 1);
        let second = queue.join("t1", 2);

        drop(first);

        assert!(timeout(SHORT, second.wait_for_turn()).await.is_ok());
    }

    #[tokio::test]
    async fn another_thread_does_not_hold_an_exchange_back() {
        // It orders exchanges only within their own thread
        let queue = Arc::<Queue>::default();
        let _other = queue.join("t1", 1);
        let mine = queue.join("t2", 2);

        assert!(timeout(SHORT, mine.wait_for_turn()).await.is_ok());
    }

    #[tokio::test]
    async fn a_waiting_exchange_is_woken_when_the_earlier_one_leaves() {
        // It goes ahead after the wait began, not only when it checks first
        let queue = Arc::<Queue>::default();
        let first = queue.join("t1", 1);
        let second = queue.join("t1", 2);
        let waiting = tokio::spawn(async move { second.wait_for_turn().await });

        tokio::time::sleep(SHORT).await;
        first.leave();

        assert!(timeout(Duration::from_secs(5), waiting).await.is_ok());
    }
}
