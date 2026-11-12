//! Text (and binary-notice) file preview.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use forge_model::EntryKind;

use crate::list::{self, FsError};

/// Page budgets, not a total file-size / line-count limit.
pub const PREVIEW_MAX_BYTES: usize = 64 * 1024;
pub const PREVIEW_MAX_LINES: usize = 400;
/// Prevent one giant source line from exhausting memory or the TUI renderer.
const PREVIEW_MAX_LINE_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone)]
pub struct TextPage {
    pub lines: Vec<String>,
    /// Absolute byte offset of the next complete source line.
    pub next_offset: u64,
    pub eof: bool,
}

/// Read a bounded page of complete source lines, without losing UTF-8
/// characters or duplicating partial lines at a page boundary.
/// A pathological giant line is scanned in bounded buffers and shown with
/// an explicit marker rather than being allocated in full.
pub async fn read_text_page(path: PathBuf, offset: u64) -> Result<TextPage, FsError> {
    use tokio::io::{AsyncBufReadExt, AsyncSeekExt};
    let file = tokio::fs::File::open(&path).await?;
    let size = file.metadata().await?.len();
    if offset > size {
        return Err(FsError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "file shrank while reading pages",
        )));
    }
    let mut reader = tokio::io::BufReader::new(file);
    reader.seek(std::io::SeekFrom::Start(offset)).await?;
    let mut lines = Vec::new();
    let mut consumed = 0usize;
    while offset.saturating_add(consumed as u64) < size
        && consumed < PREVIEW_MAX_BYTES
        && lines.len() < PREVIEW_MAX_LINES
    {
        let mut line = Vec::new();
        let mut full_len = 0usize;
        loop {
            let buffered = reader.fill_buf().await?;
            if buffered.is_empty() {
                break;
            }
            let newline = buffered.iter().position(|b| *b == b'\n');
            let take = newline.map_or(buffered.len(), |i| i + 1);
            let store = take.min(PREVIEW_MAX_LINE_BYTES.saturating_sub(line.len()));
            line.extend_from_slice(&buffered[..store]);
            full_len += take;
            consumed += take;
            reader.consume(take);
            if newline.is_some() {
                break;
            }
        }
        if full_len == 0 {
            break;
        }
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        let mut text = String::from_utf8_lossy(&line).into_owned();
        if full_len > PREVIEW_MAX_LINE_BYTES {
            text.push_str(" … [very long line shortened]");
        }
        lines.push(text);
    }
    let next_offset = offset + consumed as u64;
    Ok(TextPage {
        lines,
        next_offset,
        eof: next_offset >= size,
    })
}

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
    Binary { size: u64, notice: String },
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
    Error { message: String },
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
pub async fn preview_path(path: PathBuf, kind: EntryKind) -> Result<Preview, FsError> {
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
    use tokio::io::AsyncReadExt;
    let meta = tokio::fs::metadata(&path).await?;
    if meta.is_dir() {
        return preview_directory(path).await;
    }
    let size = meta.len();
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
    let mut sample = [0u8; SNIFF_BYTES];
    let mut file = tokio::fs::File::open(&path).await?;
    let n = size.min(SNIFF_BYTES as u64) as usize;
    file.read_exact(&mut sample[..n]).await?;
    if looks_binary(&sample[..n]) {
        return Ok(Preview {
            path,
            content: PreviewContent::Binary {
                size,
                notice: format!("binary file - {} - preview skipped", format_size(size)),
            },
        });
    }
    let page = read_text_page(path.clone(), 0).await?;
    Ok(Preview {
        path,
        content: PreviewContent::Text {
            lines: page.lines,
            truncated: !page.eof,
            bytes_read: page.next_offset as usize,
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
            PreviewContent::Text {
                lines, truncated, ..
            } => {
                assert_eq!(lines, vec!["hello", "world"]);
                assert!(!truncated);
            }
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn thousands_of_lines_stream_without_missing_or_duplicate_lines() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("many.rs");
        let source = (0..3000)
            .map(|n| format!("// 行 {n:04}  🌐 let value = {n};\r\n"))
            .collect::<String>();
        tokio::fs::write(&path, source).await.unwrap();
        let preview = preview_path(path.clone(), EntryKind::File).await.unwrap();
        let PreviewContent::Text {
            mut lines,
            mut truncated,
            mut bytes_read,
        } = preview.content
        else {
            panic!("source file should be text");
        };
        assert!(truncated);
        assert!(lines.len() <= PREVIEW_MAX_LINES);
        let mut pages = 1;
        while truncated {
            let old = bytes_read;
            let page = read_text_page(path.clone(), old as u64).await.unwrap();
            assert!(page.next_offset > old as u64);
            assert!(page.lines.len() <= PREVIEW_MAX_LINES);
            lines.extend(page.lines);
            bytes_read = page.next_offset as usize;
            truncated = !page.eof;
            pages += 1;
            assert!(pages < 100, "pagination must reach EOF");
        }
        assert_eq!(pages, 8);
        assert_eq!(lines.len(), 3000);
        assert_eq!(lines.first().unwrap(), "// 行 0000  🌐 let value = 0;");
        assert_eq!(lines.last().unwrap(), "// 行 2999  🌐 let value = 2999;");
        assert_eq!(
            bytes_read,
            tokio::fs::metadata(&path).await.unwrap().len() as usize
        );
    }

    #[tokio::test]
    async fn pagination_keeps_unicode_and_long_lines_on_exact_source_boundaries() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("utf8.rs");
        let mut source = format!(
            "{}\n{}\n{}",
            "a".repeat(20_000),
            "b".repeat(20_000),
            "c".repeat(23_000)
        );
        source.push_str("🦀");
        source.push_str("\nlast line\n");
        source.push_str(&"x".repeat(90_000));
        source.push_str("\nfinished\n");
        tokio::fs::write(&path, source).await.unwrap();

        let mut offset = 0;
        let mut lines = Vec::new();
        loop {
            let page = read_text_page(path.clone(), offset).await.unwrap();
            assert!(page.next_offset > offset);
            offset = page.next_offset;
            lines.extend(page.lines);
            if page.eof {
                break;
            }
        }
        assert!(
            lines[2].ends_with("🦀"),
            "cross-boundary UTF-8 must survive"
        );
        assert_eq!(lines[3], "last line");
        assert!(lines[4].contains("[very long line shortened]"));
        assert_eq!(lines[5], "finished");
        assert_eq!(offset, tokio::fs::metadata(&path).await.unwrap().len());
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
        tokio::fs::create_dir(dir.path().join("zebra"))
            .await
            .unwrap();
        tokio::fs::create_dir(dir.path().join("alpha"))
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("readme.txt"), "x")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("a.txt"), "y")
            .await
            .unwrap();

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
            PreviewContent::Directory {
                children,
                truncated,
                ..
            } => {
                assert!(children.is_empty());
                assert!(!truncated);
            }
            other => panic!("expected directory, got {other:?}"),
        }
    }
}
