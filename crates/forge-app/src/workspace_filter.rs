//! Shared file/search scope, derived from a read-only repository snapshot.
use super::*;
use forge_fs::workspace::SearchOptions;

#[derive(Default, Clone)]
pub(super) struct FileFilter {
    pub status: u8, // 0 all, 1 modified, 2 untracked
    pub extension: String,
    pub show_hidden: bool,
    pub excluded: String,
}

impl FileFilter {
    pub(super) fn options(
        &self,
        status: Option<&StatusSnapshot>,
        content: bool,
        regex: bool,
    ) -> SearchOptions {
        let mut options = SearchOptions {
            content,
            regex,
            show_hidden: self.show_hidden,
            extension: self.extension.clone(),
            ..Default::default()
        };
        if !self.excluded.is_empty() {
            options.excluded.extend(
                self.excluded
                    .split(',')
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .map(str::to_owned),
            );
        }
        options.allowed = self.git_scope(status);
        options
    }
    /// None means unrestricted; Some(empty) means a status filter with no matches.
    pub(super) fn git_scope(&self, status: Option<&StatusSnapshot>) -> Option<HashSet<PathBuf>> {
        if self.status == 0 {
            return None;
        }
        let wanted = self.status;
        Some(
            status
                .map(|snapshot| {
                    snapshot
                        .entries
                        .iter()
                        .filter(|(_, value)| {
                            if wanted == 2 {
                                **value == forge_git::GitStatus::Untracked
                            } else {
                                !matches!(
                                    value,
                                    forge_git::GitStatus::Clean
                                        | forge_git::GitStatus::Untracked
                                        | forge_git::GitStatus::Ignored
                                )
                            }
                        })
                        .map(|(path, _)| snapshot.repo_root.join(path))
                        .collect()
                })
                .unwrap_or_default(),
        )
    }
}

impl App {
    pub(super) fn search_options(&self, content: bool, regex: bool) -> SearchOptions {
        self.workspace_filter
            .options(self.scm.status().as_ref(), content, regex)
    }
    /// Only an accepted snapshot that changes the effective file scope
    /// may invalidate a status-filtered file list or search index.
    pub(super) fn reconcile_git_scope(&mut self, previous: Option<HashSet<PathBuf>>) {
        if self
            .workspace_filter
            .git_scope(self.scm.status().as_ref())
            != previous
        {
            self.rebuild_visible_preserving_selection();
            self.invalidate_search_cache();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_git::GitStatus;

    #[test]
    fn git_status_is_an_explicit_optional_filter_input() {
        let root = PathBuf::from("/workspace");
        let mut snapshot = StatusSnapshot::new(root.clone());
        snapshot.insert("new.txt", GitStatus::Untracked);
        snapshot.insert("modified.rs", GitStatus::Modified);
        let mut filter = FileFilter {
            extension: "rs".into(),
            excluded: "target, cache".into(),
            ..Default::default()
        };
        let options = filter.options(Some(&snapshot), true, true);
        assert!(options.allowed.is_none());
        assert_eq!(options.extension, "rs");
        assert!(options.excluded.contains("cache"));
        filter.status = 1;
        let modified = filter
            .options(Some(&snapshot), false, false)
            .allowed
            .unwrap();
        assert_eq!(modified.len(), 1);
        assert!(modified.contains(&root.join("modified.rs")));
        filter.status = 2;
        let untracked = filter
            .options(Some(&snapshot), false, false)
            .allowed
            .unwrap();
        assert_eq!(untracked.len(), 1);
        assert!(untracked.contains(&root.join("new.txt")));
        assert!(filter
            .options(None, false, false)
            .allowed
            .unwrap()
            .is_empty());
    }
}
