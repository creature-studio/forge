//! File-tree state transitions accept explicit filter and geometry inputs.
use super::*;
pub(super) struct FilesState {
    tree_root: PathBuf,
    pub(super) expanded: HashSet<PathBuf>,
    /// Directory whose unloaded descendants should be expanded automatically
    /// as one compact folder click. None when a branch or file is encountered.
    compact_expand_pending: Option<PathBuf>,
    children: HashMap<PathBuf, Vec<Entry>>,
    pub(super) visible: Vec<TreeRow>,
    pub(super) selected: usize,
    pub(super) scroll: usize,
    pub(super) follow_selection: bool,
    external_anchor: Option<(PathBuf, usize)>,
    /// Keep a deliberately revealed path and its ancestors visible even
    /// through extension/status/exclusion filters.
    revealed_path: Option<PathBuf>,
    pub(super) pending_lists: Vec<PendingList>,
    pub(super) last_list_click: Option<(Instant, usize)>,
    pub(super) sticky_headers: Vec<usize>,
}

#[derive(Clone)]
pub(super) struct FileNavigation {
    selected_path: Option<PathBuf>,
    scroll: usize,
    expanded: HashSet<PathBuf>,
}

impl FilesState {
    pub(super) fn new(tree_root: PathBuf) -> Self {
        Self {
            tree_root,
            expanded: HashSet::new(),
            compact_expand_pending: None,
            children: HashMap::new(),
            visible: Vec::new(),
            selected: 0,
            scroll: 0,
            follow_selection: true,
            external_anchor: None,
            revealed_path: None,
            pending_lists: Vec::new(),
            last_list_click: None,
            sticky_headers: Vec::new(),
        }
    }

    pub(super) fn root(&self) -> &Path {
        &self.tree_root
    }

    pub(super) fn has_listing(&self, path: &Path) -> bool {
        self.children.contains_key(path)
    }

    pub(super) fn clear_listings(&mut self) {
        self.children.clear();
    }

    /// Accept raw directory data only for the current workspace. The
    /// caller owns cancellation of superseded scheduler jobs.
    pub(super) fn install_listing(&mut self, path: PathBuf, entries: Vec<Entry>) -> bool {
        if !path.starts_with(&self.tree_root) {
            return false;
        }
        self.children.insert(path, entries);
        true
    }

    #[cfg(test)]
    pub(super) fn raw_listings(&self) -> &HashMap<PathBuf, Vec<Entry>> {
        &self.children
    }

    #[cfg(test)]
    pub(super) fn test_listings_mut(&mut self) -> &mut HashMap<PathBuf, Vec<Entry>> {
        &mut self.children
    }

    pub(super) fn navigation_snapshot(&self) -> FileNavigation {
        FileNavigation {
            selected_path: self.selected_entry().map(|entry| entry.path.clone()),
            scroll: self.scroll,
            expanded: self.expanded.clone(),
        }
    }

    pub(super) fn restore_workspace(&mut self, root: PathBuf, restore: Option<&FileNavigation>) {
        self.tree_root = root;
        self.children.clear();
        self.expanded = restore.map(|n| n.expanded.clone()).unwrap_or_default();
        self.external_anchor =
            restore.and_then(|n| n.selected_path.clone().map(|path| (path, n.scroll)));
        self.scroll = restore.map_or(0, |n| n.scroll);
        self.visible.clear();
        self.selected = 0;
        self.follow_selection = true;
        self.sticky_headers.clear();
        self.last_list_click = None;
        self.compact_expand_pending = None;
        self.revealed_path = None;
        // Pending directory jobs are canceled by App's initialize_workspace_listing before
        // any new workspace results can be accepted.
    }

    pub(super) fn record_refresh_anchor(&mut self) {
        if self.external_anchor.is_none() {
            self.external_anchor = self
                .selected_entry()
                .map(|entry| (entry.path.clone(), self.scroll));
        }
    }

    pub(super) fn refresh_anchor_path(&self) -> Option<&Path> {
        self.external_anchor
            .as_ref()
            .map(|(path, _)| path.as_path())
    }

    #[cfg(test)]
    pub(super) fn restore_workspace_anchor(&mut self, anchor: Option<(PathBuf, usize)>) {
        self.external_anchor = anchor;
    }

    pub(super) fn clear_reveal_override(&mut self) {
        self.revealed_path = None;
    }

    #[cfg(test)]
    pub(super) fn revealed_path(&self) -> Option<&Path> {
        self.revealed_path.as_deref()
    }

    pub(super) fn cancel_compact_expansion(&mut self) {
        self.compact_expand_pending = None;
    }

    pub(super) fn begin_compact_expansion(&mut self, path: PathBuf) {
        self.compact_expand_pending = Some(path);
    }

    #[cfg(test)]
    pub(super) fn compact_expansion_pending(&self) -> bool {
        self.compact_expand_pending.is_some()
    }

    pub(super) fn begin_reveal(&mut self, path: &Path) {
        self.external_anchor = None;
        self.last_list_click = None;
        self.revealed_path = Some(path.to_owned());
        for parent in path
            .ancestors()
            .skip(1)
            .take_while(|p| p.starts_with(&self.tree_root))
        {
            self.expanded.insert(parent.to_owned());
        }
    }

    pub(super) fn select_revealed_path(&mut self, path: &Path, viewport: usize) -> bool {
        let Some(index) = self.visible.iter().position(|row| row.entry.path == path) else {
            return false;
        };
        self.selected = index;
        self.follow_selection = true;
        self.scroll = index.saturating_sub(viewport.saturating_sub(1) / 2);
        self.external_anchor = None;
        true
    }

    pub(super) fn selected_entry(&self) -> Option<&Entry> {
        self.visible.get(self.selected).map(|row| &row.entry)
    }

    pub(super) fn rebuild(&mut self, options: &forge_fs::workspace::SearchOptions, compact: bool) {
        let visible = flatten_tree(&self.tree_root, &self.children, &self.expanded);
        let filtered: Vec<_> = visible
            .into_iter()
            .filter(|row| self.filter_entry(&row.entry, options))
            .collect();
        self.visible = if compact {
            compact_tree_rows(&filtered, &self.children)
        } else {
            filtered
        };
        if self.visible.is_empty() {
            self.selected = 0;
            self.scroll = 0;
        } else {
            self.selected = self.selected.min(self.visible.len() - 1);
        }
    }

    fn filter_entry(&self, entry: &Entry, options: &forge_fs::workspace::SearchOptions) -> bool {
        // An explicit Open file / Reveal request must be visible even if a
        // previous extension, status or excluded-folder filter hides its path.
        // Never bypass the special .git directory exclusion.
        if entry.name != ".git"
            && self.revealed_path.as_ref().is_some_and(|target| {
                entry.path == *target
                    || (entry.kind == EntryKind::Directory && target.starts_with(&entry.path))
            })
        {
            return true;
        }
        if entry.name == ".git"
            || (entry.kind == EntryKind::Directory && options.excluded.contains(&entry.name))
            || (!options.show_hidden && entry.name.starts_with('.'))
        {
            return false;
        }
        if let Some(allowed) = &options.allowed {
            if !allowed.iter().any(|p| {
                p == &entry.path
                    || (entry.kind == EntryKind::Directory && p.starts_with(&entry.path))
            }) {
                return false;
            }
        }
        entry.kind == EntryKind::Directory
            || options.extension.is_empty()
            || entry.path.extension().and_then(|e| e.to_str())
                == Some(options.extension.trim_start_matches('.'))
    }

    /// Rebuild after a filter change while keeping selection identity stable.
    ///
    /// The selected path wins even when its row index moved. If the path is
    /// filtered out, the nearest visible ancestor directory is selected;
    /// otherwise the previous index is clamped as a last resort. Expansion and
    /// focus are untouched so closing a filter restores the same tree shape.
    pub(super) fn rebuild_preserving_selection(
        &mut self,
        options: &forge_fs::workspace::SearchOptions,
        compact: bool,
        viewport: usize,
    ) {
        let anchor = self.selected_entry().map(|entry| entry.path.clone());
        let old_selected = self.selected;
        let old_scroll = self.scroll;
        self.rebuild(options, compact);
        let len = self.visible.len();
        if len == 0 {
            return;
        }
        let found = anchor.and_then(|path| {
            if let Some(index) = self.visible.iter().position(|row| row.entry.path == path) {
                return Some(index);
            }
            // When an expanded chain becomes compact, its original folder
            // is represented by the deepest directory in that same chain.
            // Keep the selected folder instead of jumping up to a sibling.
            if let Some(index) = self.visible.iter().position(|row| {
                matches!(row.entry.kind, EntryKind::Directory)
                    && row.entry.name.contains(" / ")
                    && row.entry.path.starts_with(&path)
            }) {
                return Some(index);
            }
            let mut parent = path.parent();
            while let Some(candidate) = parent {
                if let Some(index) = self
                    .visible
                    .iter()
                    .position(|row| row.entry.path == candidate)
                {
                    return Some(index);
                }
                if candidate == self.tree_root {
                    break;
                }
                parent = candidate.parent();
            }
            None
        });
        self.selected = found.unwrap_or_else(|| old_selected.min(len - 1));
        if !self.follow_selection {
            self.scroll = old_scroll.min(len - 1);
            return;
        }
        let max_scroll = len.saturating_sub(viewport.max(1));
        if viewport == 0 {
            self.scroll = old_scroll.min(self.selected);
        } else if self.selected >= old_scroll && self.selected < old_scroll.saturating_add(viewport)
        {
            self.scroll = old_scroll.min(max_scroll);
        } else if self.selected < old_scroll {
            self.scroll = self.selected;
        } else {
            self.scroll = self
                .selected
                .saturating_sub(viewport.saturating_sub(1))
                .min(max_scroll);
        }
    }

    pub(super) fn advance_compact_folder_expansion(&mut self) -> Option<PathBuf> {
        let mut path = self.compact_expand_pending.take()?;
        for _ in 0..128 {
            if !self.expanded.contains(&path) {
                break;
            }
            let Some(children) = self.children.get(&path) else {
                self.compact_expand_pending = Some(path.clone());
                break;
            };
            if children.len() != 1 || !matches!(children[0].kind, EntryKind::Directory) {
                break;
            }
            let next = children[0].path.clone();
            if !next.starts_with(&path) || next == path {
                break;
            }
            self.expanded.insert(next.clone());
            path = next;
        }
        Some(path)
    }

    pub(super) fn restore_refresh_anchor(&mut self) {
        if let Some((path, old_scroll)) = self.external_anchor.clone() {
            if let Some(index) = self.visible.iter().position(|row| row.entry.path == path) {
                self.selected = index;
                self.scroll = if self.follow_selection {
                    old_scroll.min(index)
                } else {
                    old_scroll
                };
                // Subsequent expanded-directory results can still shift
                // row indices. Retain the original anchor until the
                // entire refresh is complete, not just the root load.
                if self.pending_lists.is_empty() {
                    self.external_anchor = None;
                }
            } else if self.pending_lists.is_empty() {
                // Removed paths fall back to their closest visible ancestor.
                let mut parent = path.parent();
                while let Some(folder) = parent {
                    if let Some(index) =
                        self.visible.iter().position(|row| row.entry.path == folder)
                    {
                        self.selected = index;
                        break;
                    }
                    parent = folder.parent();
                }
                self.external_anchor = None;
            }
        }
    }

    pub(super) fn move_selection(&mut self, delta: isize) -> bool {
        self.compact_expand_pending = None;
        self.follow_selection = true;
        self.external_anchor = None;
        if self.visible.is_empty() {
            return false;
        }
        let next = (self.selected as isize)
            .saturating_add(delta)
            .clamp(0, self.visible.len() as isize - 1) as usize;
        let changed = next != self.selected;
        self.selected = next;
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(path: &Path, kind: EntryKind) -> Entry {
        Entry {
            id: forge_model::PathId(0),
            name: path.file_name().unwrap().to_string_lossy().into_owned(),
            path: path.to_owned(),
            kind,
        }
    }

    #[test]
    fn workspace_restore_owns_selection_and_clears_transient_gestures() {
        let root = PathBuf::from("/workspace");
        let target = root.join("a.rs");
        let mut files = FilesState::new(root.clone());
        files
            .children
            .insert(root.clone(), vec![entry(&target, EntryKind::File)]);
        files.rebuild(&Default::default(), false);
        files.expanded.insert(root.clone());
        files.scroll = 4;
        let snapshot = files.navigation_snapshot();
        files.last_list_click = Some((Instant::now(), 0));
        files.begin_compact_expansion(root.clone());
        files.begin_reveal(&target);
        files.last_list_click = Some((Instant::now(), 0));
        files.restore_workspace(PathBuf::from("/other"), None);
        assert!(files.last_list_click.is_none());
        assert!(files.revealed_path.is_none());
        assert!(files.compact_expand_pending.is_none());
        assert!(files.visible.is_empty());
        assert!(files.external_anchor.is_none());
        files.restore_workspace(root.clone(), Some(&snapshot));
        assert!(files.expanded.contains(&root));
        assert_eq!(files.external_anchor, Some((target, 4)));
        assert_eq!(files.scroll, 4);
    }

    #[test]
    fn reveal_expands_ancestors_and_selects_path_without_navigation_or_app() {
        let root = PathBuf::from("/workspace");
        let folder = root.join("src");
        let target = folder.join("a.rs");
        let mut files = FilesState::new(root.clone());
        files
            .children
            .insert(root.clone(), vec![entry(&folder, EntryKind::Directory)]);
        files
            .children
            .insert(folder.clone(), vec![entry(&target, EntryKind::File)]);
        files.external_anchor = Some((root.join("old.txt"), 9));
        files.begin_reveal(&target);
        assert!(files.expanded.contains(&root));
        assert!(files.expanded.contains(&folder));
        assert!(files.external_anchor.is_none());
        let options = forge_fs::workspace::SearchOptions {
            extension: "txt".into(),
            ..Default::default()
        };
        files.rebuild(&options, false);
        assert!(files.select_revealed_path(&target, 5));
        assert_eq!(files.selected_entry().unwrap().path, target);
        let selected = files.selected;
        assert!(!files.select_revealed_path(&root.join("missing.txt"), 5));
        assert_eq!(files.selected, selected);
    }

    #[test]
    fn filtering_and_selection_are_independent_of_app_and_io() {
        let root = PathBuf::from("/workspace");
        let mut files = FilesState::new(root.clone());
        let a = root.join("a.rs");
        let b = root.join("b.txt");
        files.children.insert(
            root,
            vec![entry(&a, EntryKind::File), entry(&b, EntryKind::File)],
        );
        let mut options = forge_fs::workspace::SearchOptions::default();
        files.rebuild(&options, false);
        assert_eq!(files.visible.len(), 2);
        assert!(files.move_selection(1));
        assert_eq!(files.selected_entry().unwrap().path, b);
        options.extension = "rs".into();
        files.rebuild_preserving_selection(&options, false, 10);
        assert_eq!(files.selected_entry().unwrap().path, a);
        options.extension.clear();
        files.rebuild_preserving_selection(&options, false, 10);
        assert_eq!(files.selected_entry().unwrap().path, a);
    }
}
