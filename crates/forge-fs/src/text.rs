//! Bounded UTF-8 text editing with atomic writes and external-change detection.
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

pub const MAX_TEXT_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct TextDocument {
    pub path: PathBuf,
    pub text: String,
    root: PathBuf,
    target: PathBuf,
    original: Vec<u8>,
    crlf: bool,
}

fn target(root: &Path, path: &Path) -> Result<PathBuf> {
    let path = super::workspace::checked_path(root, path)?;
    let resolved = path.canonicalize()?;
    super::workspace::checked_path(root, &resolved)?;
    if !resolved.is_file() {
        bail!("Select a text file to edit");
    }
    Ok(resolved)
}

fn read_text_bytes(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take((MAX_TEXT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_TEXT_BYTES {
        bail!("Text editor supports files up to 2 MiB; use the external editor (E)");
    }
    Ok(bytes)
}

pub fn load(root: &Path, path: &Path) -> Result<TextDocument> {
    let root = root.canonicalize()?;
    let path = super::workspace::checked_path(&root, path)?;
    let target = target(&root, &path)?;
    let original = read_text_bytes(&target)?;
    if original.contains(&0) {
        bail!("Binary files cannot be edited as text");
    }
    let raw = std::str::from_utf8(&original)
        .context("Text editor requires UTF-8; use E for an external editor")?;
    let crlf = raw.contains("\r\n") && !raw.replace("\r\n", "").contains('\n');
    let text = if crlf {
        raw.replace("\r\n", "\n")
    } else {
        raw.to_owned()
    };
    Ok(TextDocument {
        path,
        text,
        root,
        target,
        original,
        crlf,
    })
}

pub fn save(mut document: TextDocument, text: String) -> Result<TextDocument> {
    let bytes = if document.crlf {
        text.replace('\n', "\r\n").into_bytes()
    } else {
        text.as_bytes().to_vec()
    };
    if bytes.len() > MAX_TEXT_BYTES {
        bail!("Text exceeds the editor's 2 MiB limit");
    }
    if target(&document.root, &document.path)? != document.target {
        bail!("File target changed on disk. Draft retained; reopen the file before saving");
    }
    let previous = read_text_bytes(&document.target)?;
    if previous != document.original {
        bail!("File changed on disk. Draft retained; save externally or reopen the file");
    }
    let permissions = fs::metadata(&document.target)?.permissions();
    let mut temp =
        tempfile::NamedTempFile::new_in(document.target.parent().context("File has no parent")?)?;
    temp.write_all(&bytes)?;
    temp.as_file().set_permissions(permissions)?;
    temp.as_file().sync_all()?;
    // Recheck after writing the temporary file before replacing the original.
    if target(&document.root, &document.path)? != document.target
        || read_text_bytes(&document.target)? != document.original
    {
        bail!("File changed on disk during save. Draft retained");
    }
    temp.persist(&document.target).map_err(|e| e.error)?;
    document.original = bytes;
    document.text = text;
    Ok(document)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_unicode_crlf_without_adding_final_newline_and_preserves_permissions() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("text.txt");
        fs::write(&path, "第一行\r\nlast").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        }
        let doc = load(root.path(), &path).unwrap();
        assert_eq!(doc.text, "第一行\nlast");
        let doc = save(doc, "你好\nnext".into()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), "你好\r\nnext".as_bytes());
        save(doc, "你好\nnext\n".into()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), "你好\r\nnext\r\n".as_bytes());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o640
            );
        }
    }

    #[test]
    fn refuses_external_changes_binary_large_and_protected_files() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("text");
        fs::write(&path, "original").unwrap();
        let doc = load(root.path(), &path).unwrap();
        fs::write(&path, "external").unwrap();
        assert!(save(doc, "draft".into()).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "external");
        fs::write(&path, [0, 1, 2]).unwrap();
        assert!(load(root.path(), &path).is_err());
        fs::write(&path, vec![b'a'; MAX_TEXT_BYTES + 1]).unwrap();
        assert!(load(root.path(), &path).is_err());
        assert!(load(root.path(), root.path()).is_err());
        fs::write(root.path().join(".git"), "protected").unwrap();
        assert!(load(root.path(), &root.path().join(".git")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_outside_workspace_and_changed_targets() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("out"), "outside").unwrap();
        let link = root.path().join("link");
        symlink(outside.path().join("out"), &link).unwrap();
        assert!(load(root.path(), &link).is_err());
        fs::remove_file(&link).unwrap();
        fs::write(root.path().join("a"), "inside").unwrap();
        fs::write(root.path().join("b"), "inside").unwrap();
        symlink(root.path().join("a"), &link).unwrap();
        let doc = load(root.path(), &link).unwrap();
        fs::remove_file(&link).unwrap();
        symlink(root.path().join("b"), &link).unwrap();
        assert!(save(doc, "draft".into()).is_err());
        assert_eq!(fs::read_to_string(root.path().join("b")).unwrap(), "inside");
    }
}
