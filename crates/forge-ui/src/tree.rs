//! Expandable file-tree flatten helpers (pure, no Ratatui).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use forge_model::{Entry, EntryKind};

/// One visible row in the file tree pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    pub entry: Entry,
    pub depth: u16,
    /// Meaningful for directories; always `false` for files.
    pub expanded: bool,
}

/// Depth-first flatten of `root`'s children (and nested expanded dirs).
///
/// `root` itself is not emitted — only its children, matching IDE file trees.
pub fn flatten_tree(
    root: &Path,
    children: &HashMap<PathBuf, Vec<Entry>>,
    expanded: &HashSet<PathBuf>,
) -> Vec<TreeRow> {
    let mut out = Vec::new();
    let Some(entries) = children.get(root) else {
        return out;
    };
    for entry in entries {
        push_entry(entry, 0, children, expanded, &mut out);
    }
    out
}

fn push_entry(
    entry: &Entry,
    depth: u16,
    children: &HashMap<PathBuf, Vec<Entry>>,
    expanded: &HashSet<PathBuf>,
    out: &mut Vec<TreeRow>,
) {
    let is_dir = matches!(entry.kind, EntryKind::Directory);
    let is_expanded = is_dir && expanded.contains(&entry.path);
    out.push(TreeRow {
        entry: entry.clone(),
        depth,
        expanded: is_expanded,
    });
    if is_expanded {
        if let Some(kids) = children.get(&entry.path) {
            for kid in kids {
                push_entry(kid, depth.saturating_add(1), children, expanded, out);
            }
        }
    }
}

/// Index of the nearest ancestor row with `depth == row.depth - 1`.
pub fn find_parent_row(rows: &[TreeRow], index: usize) -> Option<usize> {
    let depth = rows.get(index)?.depth;
    if depth == 0 {
        return None;
    }
    let target = depth - 1;
    (0..index).rev().find(|&i| rows[i].depth == target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_model::PathId;

    fn entry(name: &str, path: &str, dir: bool) -> Entry {
        Entry {
            id: PathId(1),
            name: name.into(),
            path: PathBuf::from(path),
            kind: if dir {
                EntryKind::Directory
            } else {
                EntryKind::File
            },
        }
    }

    #[test]
    fn flatten_root_children_collapsed() {
        let root = PathBuf::from("/proj");
        let mut children = HashMap::new();
        children.insert(
            root.clone(),
            vec![
                entry("crates", "/proj/crates", true),
                entry("README.md", "/proj/README.md", false),
            ],
        );
        let expanded = HashSet::new();
        let rows = flatten_tree(&root, &children, &expanded);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].entry.name, "crates");
        assert_eq!(rows[0].depth, 0);
        assert!(!rows[0].expanded);
        assert_eq!(rows[1].entry.name, "README.md");
        assert_eq!(rows[1].depth, 0);
    }

    #[test]
    fn flatten_expands_nested() {
        let root = PathBuf::from("/proj");
        let mut children = HashMap::new();
        children.insert(
            root.clone(),
            vec![
                entry("crates", "/proj/crates", true),
                entry("README.md", "/proj/README.md", false),
            ],
        );
        children.insert(
            PathBuf::from("/proj/crates"),
            vec![
                entry("forge", "/proj/crates/forge", true),
                entry("forge-ui", "/proj/crates/forge-ui", true),
            ],
        );
        children.insert(
            PathBuf::from("/proj/crates/forge"),
            vec![entry("Cargo.toml", "/proj/crates/forge/Cargo.toml", false)],
        );

        let mut expanded = HashSet::new();
        expanded.insert(PathBuf::from("/proj/crates"));
        expanded.insert(PathBuf::from("/proj/crates/forge"));

        let rows = flatten_tree(&root, &children, &expanded);
        let names: Vec<_> = rows
            .iter()
            .map(|r| (r.depth, r.entry.name.as_str(), r.expanded))
            .collect();
        assert_eq!(
            names,
            vec![
                (0, "crates", true),
                (1, "forge", true),
                (2, "Cargo.toml", false),
                (1, "forge-ui", false),
                (0, "README.md", false),
            ]
        );
    }

    #[test]
    fn expanded_without_cache_still_shows_dir() {
        let root = PathBuf::from("/proj");
        let mut children = HashMap::new();
        children.insert(
            root.clone(),
            vec![entry("crates", "/proj/crates", true)],
        );
        let mut expanded = HashSet::new();
        expanded.insert(PathBuf::from("/proj/crates"));
        let rows = flatten_tree(&root, &children, &expanded);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].expanded);
    }

    #[test]
    fn find_parent_walks_up() {
        let rows = vec![
            TreeRow {
                entry: entry("crates", "/proj/crates", true),
                depth: 0,
                expanded: true,
            },
            TreeRow {
                entry: entry("forge", "/proj/crates/forge", true),
                depth: 1,
                expanded: false,
            },
            TreeRow {
                entry: entry("ui", "/proj/crates/ui", true),
                depth: 1,
                expanded: false,
            },
        ];
        assert_eq!(find_parent_row(&rows, 2), Some(0));
        assert_eq!(find_parent_row(&rows, 1), Some(0));
        assert_eq!(find_parent_row(&rows, 0), None);
    }
}
