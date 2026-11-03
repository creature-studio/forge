//! Filesystem services: listing, watching, preview, and file ops.

#![forbid(unsafe_code)]

mod list;
mod preview;

use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Duration;

use forge_jobs::{JobHandle, JobPriority, JobScheduler};
pub use forge_model::{Entry, EntryKind, PathId};
pub use list::FsError;
pub use preview::{
    preview_path, Preview, PreviewContent, PREVIEW_MAX_BYTES, PREVIEW_MAX_LINES,
};

/// Light debounce before starting a preview read (latest-wins cancels earlier jobs).
pub const PREVIEW_DEBOUNCE: Duration = Duration::from_millis(40);

/// Filesystem service facade.
#[derive(Debug, Clone)]
pub struct FsService {
    next_path_id: Arc<AtomicU64>,
}

impl Default for FsService {
    fn default() -> Self {
        Self::new()
    }
}

impl FsService {
    pub fn new() -> Self {
        Self {
            next_path_id: Arc::new(AtomicU64::new(1)),
        }
    }

    /// List a directory through the job scheduler (HIGH priority, latest-wins per path).
    pub fn list(
        &self,
        jobs: &JobScheduler,
        dir: impl Into<PathBuf>,
    ) -> JobHandle<Result<Vec<Entry>, FsError>> {
        let dir = dir.into();
        let key = format!("fs:list:{}", dir.display());
        let ids = self.next_path_id.clone();
        jobs.spawn(JobPriority::High, Some(key), async move {
            list::list_dir(dir, ids).await
        })
    }

    /// Direct async list (bypasses the scheduler; useful for tests).
    pub async fn list_now(&self, dir: impl Into<PathBuf>) -> Result<Vec<Entry>, FsError> {
        list::list_dir(dir.into(), self.next_path_id.clone()).await
    }

    /// Preview a path through the job scheduler (HIGH, single latest-wins key).
    ///
    /// Debounces briefly so rapid selection changes cancel before I/O.
    pub fn preview(
        &self,
        jobs: &JobScheduler,
        path: PathBuf,
        kind: EntryKind,
    ) -> JobHandle<Result<Preview, FsError>> {
        // One global preview key: navigating cancels the previous read.
        let key = "fs:preview".to_string();
        jobs.spawn(JobPriority::High, Some(key), async move {
            tokio::time::sleep(PREVIEW_DEBOUNCE).await;
            preview::preview_path(path, kind).await
        })
    }

    /// Direct async preview (tests / sync callers).
    pub async fn preview_now(
        &self,
        path: impl Into<PathBuf>,
        kind: EntryKind,
    ) -> Result<Preview, FsError> {
        preview::preview_path(path.into(), kind).await
    }
}
