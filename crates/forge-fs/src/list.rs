//! Async directory listing.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use forge_model::{Entry, EntryKind, PathId};

/// Filesystem errors.
#[derive(Debug, thiserror::Error)]
pub enum FsError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a directory: {0}")]
    NotADirectory(PathBuf),
}

/// Read a complete directory within a single blocking worker.
///
/// Tokio's async read_dir/file_type previously scheduled two filesystem
/// operations *per entry*. On very wide project roots this meant thousands
/// of worker handoffs before the first Files row could be shown.
pub(crate) async fn list_dir(dir: PathBuf, next_id: Arc<AtomicU64>) -> Result<Vec<Entry>, FsError> {
    tokio::task::spawn_blocking(move || list_dir_blocking(dir, next_id))
        .await
        .map_err(|err| FsError::Io(std::io::Error::other(format!("directory worker: {err}"))))?
}

fn list_dir_blocking(dir: PathBuf, next_id: Arc<AtomicU64>) -> Result<Vec<Entry>, FsError> {
    if !std::fs::metadata(&dir)?.is_dir() {
        return Err(FsError::NotADirectory(dir));
    }
    let mut entries = Vec::new();
    for item in std::fs::read_dir(&dir)? {
        let ent = item?;
        let name = ent.file_name().to_string_lossy().into_owned();
        let path = ent.path();
        let file_type = ent.file_type()?;
        let kind = if file_type.is_dir() {
            EntryKind::Directory
        } else if file_type.is_symlink() {
            EntryKind::Symlink
        } else if file_type.is_file() {
            EntryKind::File
        } else {
            EntryKind::Other
        };
        entries.push(Entry {
            id: PathId(next_id.fetch_add(1, Ordering::Relaxed)),
            name,
            path,
            kind,
        });
    }
    entries.sort_by(|a, b| {
        let dir_first = |k: EntryKind| !matches!(k, EntryKind::Directory);
        dir_first(a.kind)
            .cmp(&dir_first(b.kind))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use std::sync::Arc;

    #[tokio::test]
    async fn lists_workspace_dir() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let entries = list_dir(dir, Arc::new(AtomicU64::new(1))).await.unwrap();
        assert!(entries.iter().any(|e| e.name == "Cargo.toml"));
        assert!(entries
            .iter()
            .any(|e| e.name == "crates" && e.kind == EntryKind::Directory));
    }
}
