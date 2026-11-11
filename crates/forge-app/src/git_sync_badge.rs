//! Git upstream divergence shown in the workspace status bar.
//!
//! Reuse the first line of git status --short --branch already retrieved by
//! request_sync_status. No network fetch, shell calls or guessed zeros.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GitSyncBadge {
    pub branch: String,
    pub upstream: Option<(u64, u64)>, // (behind/down, ahead/up)
    pub upstream_missing: bool,
}

impl GitSyncBadge {
    pub fn parse(branch: &str, sync: &str) -> Option<Self> {
        if branch.is_empty() {
            return None;
        }
        let sync = sync.strip_prefix("## ").unwrap_or(sync).trim();
        let (upstream, upstream_missing) = match sync.split_once("...") {
            Some((name, trailing)) if name == branch => {
                if trailing.contains("[gone]") {
                    (None, true)
                } else {
                    // git status emits brackets only when at least one count
                    // is nonzero. Tracking with no brackets means ↓0 ↑0.
                    let mut behind = 0;
                    let mut ahead = 0;
                    if let Some(brackets) = trailing
                        .split_once('[')
                        .and_then(|(_, s)| s.split_once(']'))
                    {
                        for part in brackets.0.split(',').map(str::trim) {
                            if let Some(v) = part.strip_prefix("behind ") {
                                behind = v.parse::<u64>().ok()?;
                            } else if let Some(v) = part.strip_prefix("ahead ") {
                                ahead = v.parse::<u64>().ok()?;
                            }
                        }
                    }
                    (Some((behind, ahead)), false)
                }
            }
            _ => (None, false),
        };
        Some(Self {
            branch: branch.to_owned(),
            upstream,
            upstream_missing,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interprets_upstream_divergence_without_guessing() {
        assert_eq!(
            GitSyncBadge::parse("dev", "dev...origin/dev [ahead 1]"),
            Some(GitSyncBadge {
                branch: "dev".into(),
                upstream: Some((0, 1)),
                upstream_missing: false
            })
        );
        assert_eq!(
            GitSyncBadge::parse(
                "feature/x",
                "feature/x...origin/feature/x [behind 9, ahead 2]"
            )
            .unwrap()
            .upstream,
            Some((9, 2))
        );
        assert_eq!(
            GitSyncBadge::parse("main", "main...origin/main")
                .unwrap()
                .upstream,
            Some((0, 0))
        );
        assert_eq!(
            GitSyncBadge::parse("main", "main...origin/main [gone]")
                .unwrap()
                .upstream,
            None
        );
        assert!(
            GitSyncBadge::parse("main", "main...origin/main [gone]")
                .unwrap()
                .upstream_missing
        );
        assert_eq!(GitSyncBadge::parse("new", "new").unwrap().upstream, None);
        assert_eq!(
            GitSyncBadge::parse("dev", "old...origin/old [ahead 20]")
                .unwrap()
                .upstream,
            None
        );
        assert_eq!(GitSyncBadge::parse("dev", "").unwrap().upstream, None);
        assert_eq!(
            GitSyncBadge::parse("HEAD", "HEAD (no branch)")
                .unwrap()
                .upstream,
            None
        );
        assert_eq!(GitSyncBadge::parse("", "dev...origin/dev [ahead 1]"), None);
    }
}
