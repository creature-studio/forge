//! Priority job queue (HIGH / NORMAL / LOW) with cancellation and latest-wins keys.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, oneshot};
use tokio::task::AbortHandle;

pub use forge_model::{JobId, JobPriority};

/// Errors produced by the scheduler / a cancelled job.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum JobError {
    #[error("job cancelled")]
    Cancelled,
    #[error("scheduler shut down")]
    Shutdown,
}

/// Handle returned from [`JobScheduler::spawn`].
#[derive(Debug)]
pub struct JobHandle<T> {
    pub id: JobId,
    rx: oneshot::Receiver<Result<T, JobError>>,
}

impl<T> JobHandle<T> {
    /// Wait for the job to finish (or be cancelled).
    pub async fn join(self) -> Result<T, JobError> {
        match self.rx.await {
            Ok(inner) => inner,
            Err(_) => Err(JobError::Cancelled),
        }
    }

    /// Non-blocking poll. `None` means still pending.
    pub fn try_join(&mut self) -> Option<Result<T, JobError>> {
        match self.rx.try_recv() {
            Ok(inner) => Some(inner),
            Err(oneshot::error::TryRecvError::Empty) => None,
            Err(oneshot::error::TryRecvError::Closed) => Some(Err(JobError::Cancelled)),
        }
    }
}

#[derive(Clone, Default)]
struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

struct JobRecord {
    abort: Option<AbortHandle>,
    cancel: CancelFlag,
    key: Option<String>,
}

struct JobEnvelope {
    id: JobId,
    cancel: CancelFlag,
    fut: Pin<Box<dyn Future<Output = ()> + Send>>,
}

struct Inner {
    next_id: AtomicU64,
    high_tx: mpsc::UnboundedSender<JobEnvelope>,
    normal_tx: mpsc::UnboundedSender<JobEnvelope>,
    low_tx: mpsc::UnboundedSender<JobEnvelope>,
    registry: Mutex<HashMap<JobId, JobRecord>>,
    keys: Mutex<HashMap<String, JobId>>,
}

/// Tokio-backed priority scheduler.
#[derive(Clone)]
pub struct JobScheduler {
    inner: Arc<Inner>,
}

impl Default for JobScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl JobScheduler {
    /// Create a scheduler and start the priority dispatcher.
    pub fn new() -> Self {
        let (high_tx, high_rx) = mpsc::unbounded_channel();
        let (normal_tx, normal_rx) = mpsc::unbounded_channel();
        let (low_tx, low_rx) = mpsc::unbounded_channel();

        let inner = Arc::new(Inner {
            next_id: AtomicU64::new(1),
            high_tx,
            normal_tx,
            low_tx,
            registry: Mutex::new(HashMap::new()),
            keys: Mutex::new(HashMap::new()),
        });

        spawn_dispatcher(inner.clone(), high_rx, normal_rx, low_rx);

        Self { inner }
    }

    /// Submit a future at `priority`.
    ///
    /// If `key` is `Some`, any previous job with the same key is cancelled
    /// (latest-wins).
    pub fn spawn<T, F>(&self, priority: JobPriority, key: Option<String>, fut: F) -> JobHandle<T>
    where
        T: Send + 'static,
        F: Future<Output = T> + Send + 'static,
    {
        let id = JobId(self.inner.next_id.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = oneshot::channel();
        let cancel = CancelFlag::new();

        if let Some(ref k) = key {
            let prev = {
                let mut keys = self.inner.keys.lock().expect("keys lock");
                keys.insert(k.clone(), id)
            };
            if let Some(prev_id) = prev {
                self.cancel(prev_id);
            }
        }

        let cancel_run = cancel.clone();
        let wrapped = async move {
            if cancel_run.is_cancelled() {
                let _ = tx.send(Err(JobError::Cancelled));
                return;
            }
            let value = fut.await;
            if cancel_run.is_cancelled() {
                let _ = tx.send(Err(JobError::Cancelled));
                return;
            }
            let _ = tx.send(Ok(value));
        };

        {
            let mut reg = self.inner.registry.lock().expect("registry lock");
            reg.insert(
                id,
                JobRecord {
                    abort: None,
                    cancel: cancel.clone(),
                    key,
                },
            );
        }

        let envelope = JobEnvelope {
            id,
            cancel,
            fut: Box::pin(wrapped),
        };

        let tx_queue = match priority {
            JobPriority::High => &self.inner.high_tx,
            JobPriority::Normal => &self.inner.normal_tx,
            JobPriority::Low => &self.inner.low_tx,
        };
        let _ = tx_queue.send(envelope);

        JobHandle { id, rx }
    }

    /// Cancel a job by id (queued or running).
    pub fn cancel(&self, id: JobId) {
        let mut reg = self.inner.registry.lock().expect("registry lock");
        if let Some(record) = reg.get_mut(&id) {
            record.cancel.cancel();
            if let Some(abort) = record.abort.take() {
                abort.abort();
            }
            if let Some(key) = record.key.clone() {
                let mut keys = self.inner.keys.lock().expect("keys lock");
                if keys.get(&key) == Some(&id) {
                    keys.remove(&key);
                }
            }
        }
    }

    /// Cancel the active job for a logical key, if any.
    pub fn cancel_key(&self, key: &str) {
        let id = {
            let keys = self.inner.keys.lock().expect("keys lock");
            keys.get(key).copied()
        };
        if let Some(id) = id {
            self.cancel(id);
        }
    }
}

fn spawn_dispatcher(
    inner: Arc<Inner>,
    mut high_rx: mpsc::UnboundedReceiver<JobEnvelope>,
    mut normal_rx: mpsc::UnboundedReceiver<JobEnvelope>,
    mut low_rx: mpsc::UnboundedReceiver<JobEnvelope>,
) {
    tokio::spawn(async move {
        loop {
            let job = tokio::select! {
                biased;
                Some(job) = high_rx.recv() => job,
                Some(job) = normal_rx.recv() => job,
                Some(job) = low_rx.recv() => job,
                else => break,
            };

            let id = job.id;
            if job.cancel.is_cancelled() {
                let mut reg = inner.registry.lock().expect("registry lock");
                reg.remove(&id);
                continue;
            }

            let cancel = job.cancel.clone();
            let fut = job.fut;
            let handle = tokio::spawn(async move {
                if cancel.is_cancelled() {
                    return;
                }
                fut.await;
            });

            {
                let mut reg = inner.registry.lock().expect("registry lock");
                if let Some(record) = reg.get_mut(&id) {
                    if record.cancel.is_cancelled() {
                        handle.abort();
                        reg.remove(&id);
                        continue;
                    }
                    record.abort = Some(handle.abort_handle());
                } else {
                    handle.abort();
                    continue;
                }
            }

            let inner_done = inner.clone();
            tokio::spawn(async move {
                let _ = handle.await;
                let mut reg = inner_done.registry.lock().expect("registry lock");
                if let Some(record) = reg.remove(&id) {
                    if let Some(key) = record.key {
                        let mut keys = inner_done.keys.lock().expect("keys lock");
                        if keys.get(&key) == Some(&id) {
                            keys.remove(&key);
                        }
                    }
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn runs_job_and_returns_value() {
        let sched = JobScheduler::new();
        let handle = sched.spawn(JobPriority::Normal, None, async { 42u32 });
        assert_eq!(handle.join().await.unwrap(), 42);
    }

    #[tokio::test]
    async fn latest_wins_cancels_previous() {
        let sched = JobScheduler::new();
        let first = sched.spawn(
            JobPriority::Normal,
            Some("k".into()),
            async {
                tokio::time::sleep(Duration::from_millis(200)).await;
                1u32
            },
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
        let second = sched.spawn(JobPriority::Normal, Some("k".into()), async { 2u32 });
        assert_eq!(second.join().await.unwrap(), 2);
        assert_eq!(first.join().await.unwrap_err(), JobError::Cancelled);
    }
}
