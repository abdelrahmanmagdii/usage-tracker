//! Collapses bursts of "the numbers changed" signals into as few refreshes as
//! still show the latest numbers.
//!
//! The Codex App Server sends `account/rateLimits/updated` (and
//! `account/updated`) after every turn, and in bursts while an agent is busy.
//! Each one used to start its own full refresh: three requests, an alert
//! check, a state event and a tray repaint, queued one behind another on the
//! refresh lock. Now at most one refresh runs, at most one more is queued
//! behind it, and a short debounce folds a burst into a single refresh.

use std::{future::Future, sync::Mutex};

use tokio::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Phase {
    /// No worker exists.
    #[default]
    Idle,
    /// A worker exists and owes a refresh that has not started reading yet,
    /// so any further change will be seen by it.
    Pending,
    /// A refresh is in flight and may already have read the old numbers.
    Running,
}

#[derive(Debug, Default)]
pub struct RefreshCoalescer {
    phase: Mutex<Phase>,
}

impl RefreshCoalescer {
    /// Records that the data changed. Returns `true` when no worker is
    /// active, in which case the caller must start one with [`Self::drain`];
    /// otherwise the active worker is guaranteed to refresh again after this
    /// call.
    pub fn signal(&self) -> bool {
        let mut phase = self.lock();
        match *phase {
            Phase::Idle => {
                *phase = Phase::Pending;
                true
            }
            // The queued refresh has not read anything yet; it covers this
            // change as well.
            Phase::Pending => false,
            // The refresh in flight may have read the numbers before this
            // change, so exactly one follow-up is queued behind it.
            Phase::Running => {
                *phase = Phase::Pending;
                false
            }
        }
    }

    /// Runs refreshes until no change is left unseen. Each round waits
    /// `debounce` first, so the rest of a burst lands before anything is read.
    pub async fn drain<F, Fut>(&self, debounce: Duration, mut refresh: F)
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = ()>,
    {
        // A refresh that panics, or a worker dropped at shutdown, must not
        // leave the phase stuck at Running: every later signal would then
        // assume a worker exists and the meter would stop following Codex.
        let reset = ResetOnDrop(self);
        loop {
            tokio::time::sleep(debounce).await;
            *self.lock() = Phase::Running;
            refresh().await;
            let mut phase = self.lock();
            if *phase != Phase::Pending {
                *phase = Phase::Idle;
                break;
            }
        }
        std::mem::forget(reset);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Phase> {
        self.phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

struct ResetOnDrop<'a>(&'a RefreshCoalescer);

impl Drop for ResetOnDrop<'_> {
    fn drop(&mut self) {
        *self.0.lock() = Phase::Idle;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use tokio::sync::Notify;

    const DEBOUNCE: Duration = Duration::from_millis(20);

    #[tokio::test]
    async fn a_burst_becomes_one_refresh() {
        let coalescer = Arc::new(RefreshCoalescer::default());
        let runs = Arc::new(AtomicUsize::new(0));
        assert!(coalescer.signal());
        for _ in 0..50 {
            assert!(!coalescer.signal(), "only the first signal starts a worker");
        }
        let counter = runs.clone();
        coalescer
            .drain(DEBOUNCE, || {
                let counter = counter.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                }
            })
            .await;
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        // Idle again: the next change starts a new worker.
        assert!(coalescer.signal());
    }

    #[tokio::test]
    async fn signals_during_a_refresh_queue_exactly_one_follow_up() {
        let coalescer = Arc::new(RefreshCoalescer::default());
        let runs = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        assert!(coalescer.signal());

        let worker = {
            let coalescer = coalescer.clone();
            let runs = runs.clone();
            let started = started.clone();
            let release = release.clone();
            tokio::spawn(async move {
                coalescer
                    .drain(DEBOUNCE, || {
                        let runs = runs.clone();
                        let started = started.clone();
                        let release = release.clone();
                        async move {
                            // Only the first refresh is held open.
                            if runs.fetch_add(1, Ordering::SeqCst) == 0 {
                                started.notify_one();
                                release.notified().await;
                            }
                        }
                    })
                    .await;
            })
        };

        started.notified().await;
        for _ in 0..20 {
            assert!(!coalescer.signal(), "a worker is already active");
        }
        release.notify_one();
        worker.await.expect("worker");
        assert_eq!(runs.load(Ordering::SeqCst), 2);
        assert!(coalescer.signal());
    }

    #[tokio::test]
    async fn a_refresh_with_no_new_signal_does_not_repeat() {
        let coalescer = RefreshCoalescer::default();
        let runs = AtomicUsize::new(0);
        assert!(coalescer.signal());
        coalescer
            .drain(Duration::ZERO, || {
                runs.fetch_add(1, Ordering::SeqCst);
                async {}
            })
            .await;
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_dropped_worker_does_not_wedge_later_signals() {
        let coalescer = Arc::new(RefreshCoalescer::default());
        assert!(coalescer.signal());
        let worker = {
            let coalescer = coalescer.clone();
            tokio::spawn(async move {
                coalescer
                    .drain(Duration::ZERO, std::future::pending::<()>)
                    .await;
            })
        };
        // Let the worker reach the refresh that never finishes, then cancel it
        // the way a runtime shutdown would.
        while *coalescer.lock() != Phase::Running {
            tokio::task::yield_now().await;
        }
        worker.abort();
        let _ = worker.await;
        assert!(coalescer.signal(), "the next change starts a new worker");
    }
}
