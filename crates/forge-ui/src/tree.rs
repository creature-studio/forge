//! Expandable file-tree flatten helpers (pure, no Ratatui).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use forge_model::{Entry, EntryKind};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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

/// Merge loaded expanded chains containing exactly one actual subdirectory.
/// The last directory keeps its real path and expansion state; sibling files
/// (even if filtered from view) prevent merging, protecting folder actions.
pub fn compact_tree_rows(
    rows: &[TreeRow],
    children: &HashMap<PathBuf, Vec<Entry>>,
) -> Vec<TreeRow> {
    let mut result = Vec::with_capacity(rows.len());
    let mut active: Vec<(u16, u16)> = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        let depth = rows[i].depth;
        while active.last().is_some_and(|(bottom, _)| depth <= *bottom) {
            active.pop();
        }
        let removed = active.iter().map(|(_, n)| *n as u32).sum::<u32>() as u16;
        let mut last = i;
        let mut label = rows[i].entry.name.clone();
        while let Some(next) = rows.get(last + 1) {
            let parent = &rows[last];
            if !parent.expanded
                || !matches!(parent.entry.kind, EntryKind::Directory)
                || !matches!(next.entry.kind, EntryKind::Directory)
                || next.depth != parent.depth.saturating_add(1)
                || !children
                    .get(&parent.entry.path)
                    .is_some_and(|kids| kids.len() == 1 && kids[0].path == next.entry.path)
            {
                break;
            }
            last += 1;
            label.push_str(" / ");
            label.push_str(&next.entry.name);
        }
        let mut visible = rows[last].clone();
        visible.depth = depth.saturating_sub(removed);
        if last > i {
            visible.entry.name = label;
            active.push((rows[last].depth, rows[last].depth - depth));
        }
        result.push(visible);
        i = last + 1;
    }
    result
}

/// Compact paths should retain their first and last components when the
/// terminal sidebar is narrow. Unicode cell widths, not bytes, are counted.
pub(crate) fn fit_compact_label(name: &str, max: usize) -> String {
    if name.width() <= max {
        return name.to_string();
    }
    if max == 0 {
        return String::new();
    }
    if let (Some(first), Some(last)) = (name.split(" / ").next(), name.rsplit(" / ").next()) {
        if first != last {
            let middle = " / … / ";
            if first.width() + middle.width() + last.width() <= max {
                return format!("{first}{middle}{last}");
            }
        }
    }
    let mut text = String::new();
    let mut remaining = max.saturating_sub(1);
    for ch in name.chars() {
        let width = ch.width().unwrap_or(0);
        if width > remaining {
            break;
        }
        text.push(ch);
        remaining -= width;
    }
    text.push('…');
    text
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
    fn compact_path_label_keeps_visible_end_in_narrow_sidebar() {
        assert_eq!(
            fit_compact_label("zenk-admin / src / main / java / com / system", 27),
            "zenk-admin / … / system"
        );
        assert_eq!(
            fit_compact_label("中文 / java / 文件夹", 22),
            "中文 / java / 文件夹"
        );
        assert!(fit_compact_label("src / docs", 5).width() <= 5);
    }

    #[test]
    fn compact_unbranched_directory_chain_but_preserve_branching_and_real_path() {
        let root = PathBuf::from("/proj");
        let agent = PathBuf::from("/proj/agent");
        let app = agent.join("app");
        let src = app.join("src");
        let mut children = HashMap::new();
        children.insert(
            root.clone(),
            vec![
                entry("agent", "/proj/agent", true),
                entry("README.md", "/proj/README.md", false),
            ],
        );
        children.insert(agent.clone(), vec![entry("app", "/proj/agent/app", true)]);
        children.insert(app.clone(), vec![entry("src", "/proj/agent/app/src", true)]);
        children.insert(
            src.clone(),
            vec![
                entry("main.rs", "/proj/agent/app/src/main.rs", false),
                entry("tests", "/proj/agent/app/src/tests", true),
            ],
        );
        let expanded = HashSet::from([agent.clone(), app.clone(), src.clone()]);
        let raw = flatten_tree(&root, &children, &expanded);
        assert_eq!(raw.len(), 6);
        let compact = compact_tree_rows(&raw, &children);
        assert_eq!(compact.len(), 4);
        assert_eq!(compact[0].entry.name, "agent / app / src");
        assert_eq!(compact[0].entry.path, src);
        assert_eq!(compact[0].depth, 0);
        assert!(compact[0].expanded);
        assert_eq!(compact[1].entry.name, "main.rs");
        assert_eq!(compact[1].depth, 1);
        assert_eq!(compact[2].entry.name, "tests");
        assert_eq!(compact[2].depth, 1);
        assert_eq!(compact[3].entry.name, "README.md");
        assert_eq!(compact[3].depth, 0);
        assert_eq!(find_parent_row(&compact, 1), Some(0));

        // A filtered-out file still makes the physical scope branch, so
        // a directory action on the joined row remains safe.
        children
            .get_mut(&app)
            .unwrap()
            .push(entry(".env", "/proj/agent/app/.env", false));
        let compact = compact_tree_rows(&raw, &children);
        assert_eq!(compact[0].entry.name, "agent / app");
        assert_eq!(compact[0].entry.path, app);
        assert_eq!(compact[1].entry.name, "src");
        assert_eq!(compact[1].depth, 1);
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
        children.insert(root.clone(), vec![entry("crates", "/proj/crates", true)]);
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
