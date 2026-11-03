//! Text (and binary-notice) file preview.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use forge_model::EntryKind;

use crate::list::{self, FsError};

/// Soft caps for MVP preview reads.
pub const PREVIEW_MAX_BYTES: usize = 64 * 1024;
pub const PREVIEW_MAX_LINES: usize = 400;
/// Cap for directory child listings in the preview pane.
pub const DIR_PREVIEW_MAX_CHILDREN: usize = 80;
/// Sample size used for binary sniffing.
const SNIFF_BYTES: usize = 8 * 1024;

/// What the preview pane should show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewContent {
    /// UTF-8 (lossy) text lines, optionally truncated.
    Text {
        lines: Vec<String>,
        truncated: bool,
        bytes_read: usize,
    },
    /// Non-text / binary file — do not dump hex yet.
    Binary {
        size: u64,
        notice: String,
    },
    /// Selected entry is a directory (or symlink-to-dir style).
    Directory {
        name: String,
        /// `(is_dir, name)` children, dirs first then name (same as file list).
        children: Vec<(bool, String)>,
        /// True when more than [`DIR_PREVIEW_MAX_CHILDREN`] entries exist.
        truncated: bool,
    },
    /// Empty selection / nothing to show.
    Empty,
    /// I/O or other failure.
    Error {
        message: String,
    },
}

/// Preview payload tied to an absolute path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    pub path: PathBuf,
    pub content: PreviewContent,
}

impl Preview {
    pub fn empty() -> Self {
        Self {
            path: PathBuf::new(),
            content: PreviewContent::Empty,
        }
    }

    pub fn directory(
        path: PathBuf,
        name: impl Into<String>,
        children: Vec<(bool, String)>,
        truncated: bool,
    ) -> Self {
        Self {
            path,
            content: PreviewContent::Directory {
                name: name.into(),
                children,
                truncated,
            },
        }
    }

    pub fn error(path: PathBuf, message: impl Into<String>) -> Self {
        Self {
            path,
            content: PreviewContent::Error {
                message: message.into(),
            },
        }
    }
}

/// Read a capped preview for `path`.
///
/// Directories return a short notice with child names; binary files return a notice without dumping.
pub async fn preview_path(
    path: PathBuf,
    kind: EntryKind,
) -> Result<Preview, FsError> {
    match kind {
        EntryKind::Directory => preview_directory(path).await,
        EntryKind::Other => Ok(Preview {
            path,
            content: PreviewContent::Binary {
                size: 0,
                notice: "unsupported entry type".into(),
            },
        }),
        EntryKind::File | EntryKind::Symlink => preview_file(path).await,
    }
}

async fn preview_directory(path: PathBuf) -> Result<Preview, FsError> {
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let (children, truncated) = list_dir_children(&path).await?;
    Ok(Preview::directory(path, name, children, truncated))
}

/// List directory children as `(is_dir, name)`, dirs first then name (case-insensitive).
async fn list_dir_children(dir: &Path) -> Result<(Vec<(bool, String)>, bool), FsError> {
    // Reuse the shared list sorter; PathIds are discarded for preview.
    let entries = list::list_dir(dir.to_path_buf(), Arc::new(AtomicU64::new(1))).await?;
    let truncated = entries.len() > DIR_PREVIEW_MAX_CHILDREN;
    let children = entries
        .into_iter()
        .take(DIR_PREVIEW_MAX_CHILDREN)
        .map(|e| (matches!(e.kind, EntryKind::Directory), e.name))
        .collect();
    Ok((children, truncated))
}

async fn preview_file(path: PathBuf) -> Result<Preview, FsError> {
    let meta = tokio::fs::metadata(&path).await?;
    if meta.is_dir() {
        return preview_directory(path).await;
    }

    let size = meta.len();
    let file = tokio::fs::File::open(&path).await?;
    use tokio::io::AsyncReadExt;
    let mut reader = tokio::io::BufReader::new(file);

    let mut buf = vec![0u8; PREVIEW_MAX_BYTES.min(size as usize).max(1).min(PREVIEW_MAX_BYTES)];
    if size == 0 {
        return Ok(Preview {
            path,
            content: PreviewContent::Text {
                lines: Vec::new(),
                truncated: false,
                bytes_read: 0,
            },
        });
    }

    let n = reader.read(&mut buf).await?;
    buf.truncate(n);

    if looks_binary(&buf) {
        return Ok(Preview {
            path,
            content: PreviewContent::Binary {
                size,
                notice: format!(
                    "binary file - {} - preview skipped",
                    format_size(size)
                ),
            },
        });
    }

    let truncated = (n as u64) < size || n >= PREVIEW_MAX_BYTES;
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    // Preserve a trailing empty line if the file ended with \n and we still have room.
    if text.ends_with('\n') && lines.len() < PREVIEW_MAX_LINES {
        // `lines()` drops the final empty; UI doesn't need an extra blank for MVP.
    }
    let line_trunc = lines.len() > PREVIEW_MAX_LINES;
    if line_trunc {
        lines.truncate(PREVIEW_MAX_LINES);
    }

    Ok(Preview {
        path,
        content: PreviewContent::Text {
            lines,
            truncated: truncated || line_trunc,
            bytes_read: n,
        },
    })
}

fn looks_binary(sample: &[u8]) -> bool {
    let sniff = &sample[..sample.len().min(SNIFF_BYTES)];
    if sniff.is_empty() {
        return false;
    }
    // NUL is a strong binary signal.
    if sniff.contains(&0) {
        return true;
    }
    // High ratio of non-text control bytes (exclude tab/lf/cr).
    let mut weird = 0usize;
    for &b in sniff {
        if b < 0x09 || (b > 0x0d && b < 0x20) {
            weird += 1;
        }
    }
    // More than ~2% control bytes → treat as binary.
    weird * 50 > sniff.len()
}

fn format_size(size: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if size >= GB {
        format!("{:.1} GiB", size as f64 / GB as f64)
    } else if size >= MB {
        format!("{:.1} MiB", size as f64 / MB as f64)
    } else if size >= KB {
        format!("{:.1} KiB", size as f64 / KB as f64)
    } else {
        format!("{size} B")
    }
}

/// Sync helper for tests / callers that already have bytes.
#[allow(dead_code)]
pub fn preview_from_bytes(path: &Path, bytes: &[u8], total_size: u64) -> Preview {
    if looks_binary(bytes) {
        return Preview {
            path: path.to_path_buf(),
            content: PreviewContent::Binary {
                size: total_size,
                notice: format!(
                    "binary file - {} - preview skipped",
                    format_size(total_size)
                ),
            },
        };
    }
    let truncated = (bytes.len() as u64) < total_size || bytes.len() >= PREVIEW_MAX_BYTES;
    let text = String::from_utf8_lossy(bytes);
    let mut lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    let line_trunc = lines.len() > PREVIEW_MAX_LINES;
    if line_trunc {
        lines.truncate(PREVIEW_MAX_LINES);
    }
    Preview {
        path: path.to_path_buf(),
        content: PreviewContent::Text {
            lines,
            truncated: truncated || line_trunc,
            bytes_read: bytes.len(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn previews_utf8_text() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("hi.txt");
        tokio::fs::write(&path, "hello\nworld\n").await.unwrap();
        let preview = preview_path(path.clone(), EntryKind::File).await.unwrap();
        match preview.content {
            PreviewContent::Text { lines, truncated, .. } => {
                assert_eq!(lines, vec!["hello", "world"]);
                assert!(!truncated);
            }
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn detects_binary_nul() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("bin.dat");
        tokio::fs::write(&path, b"abc\0def").await.unwrap();
        let preview = preview_path(path, EntryKind::File).await.unwrap();
        assert!(matches!(preview.content, PreviewContent::Binary { .. }));
    }

    #[tokio::test]
    async fn directory_lists_children_dirs_first() {
        let dir = tempdir().unwrap();
        tokio::fs::create_dir(dir.path().join("zebra")).await.unwrap();
        tokio::fs::create_dir(dir.path().join("alpha")).await.unwrap();
        tokio::fs::write(dir.path().join("readme.txt"), "x").await.unwrap();
        tokio::fs::write(dir.path().join("a.txt"), "y").await.unwrap();

        let preview = preview_path(dir.path().to_path_buf(), EntryKind::Directory)
            .await
            .unwrap();
        match preview.content {
            PreviewContent::Directory {
                name,
                children,
                truncated,
            } => {
                assert!(!truncated);
                // tempfile dirs are named randomly; name is the leaf.
                assert!(!name.is_empty());
                assert_eq!(
                    children,
                    vec![
                        (true, "alpha".into()),
                        (true, "zebra".into()),
                        (false, "a.txt".into()),
                        (false, "readme.txt".into()),
                    ]
                );
            }
            other => panic!("expected directory, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn directory_empty_children() {
        let dir = tempdir().unwrap();
        let preview = preview_path(dir.path().to_path_buf(), EntryKind::Directory)
            .await
            .unwrap();
        match preview.content {
            PreviewContent::Directory { children, truncated, .. } => {
                assert!(children.is_empty());
                assert!(!truncated);
            }
            other => panic!("expected directory, got {other:?}"),
        }
    }
}
