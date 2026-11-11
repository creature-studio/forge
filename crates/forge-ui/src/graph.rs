//! Commit lane layout shared by the Git sidebar history section.
//!
//! The layout is computed from [`GraphCommit`] parents only, so it can be
//! unit-tested without a repository.

use std::collections::{BTreeMap, HashSet};

use forge_model::{CommitChange, CommitChangeKind, GraphCommit};
use forge_theme::{Rgb, Theme};

/// Each lane uses a glyph column plus a spacer column.
pub const LANE_WIDTH: usize = 2;

/// One glyph in the lane area of a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphCell {
    pub ch: char,
    /// Lane color index (see [`lane_color`]).
    pub color: u8,
}

/// Lane glyphs for one commit row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphRow {
    pub cells: Vec<GraphCell>,
}

/// Lane layout for a list of commits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphLayout {
    pub rows: Vec<GraphRow>,
    /// Widest lane count across all rows.
    pub lane_count: usize,
}

impl GraphLayout {
    /// Width in terminal cells the lane area needs.
    pub fn width(&self) -> u16 {
        (self.lane_count * LANE_WIDTH) as u16
    }

    /// Glyphs for a commit row, padded to [`Self::width`].
    pub fn row(&self, index: usize) -> &[GraphCell] {
        self.rows.get(index).map_or(&[], |row| row.cells.as_slice())
    }
}

/// Assign lanes for `commits`, which must be ordered children-before-parents.
pub fn layout_graph(commits: &[GraphCommit]) -> GraphLayout {
    let mut active: Vec<Option<String>> = Vec::new();
    let mut colors: Vec<u8> = Vec::new();
    let mut color_counter: u8 = 0;
    let mut rows: Vec<GraphRow> = Vec::with_capacity(commits.len());
    let mut lane_count = 0usize;
    // Parents outside the (possibly filtered) slice must not leave open lanes.
    let ids: HashSet<&str> = commits.iter().map(|commit| commit.id.as_str()).collect();

    for commit in commits {
        let mut cells: Vec<Option<GraphCell>> = vec![None; active.len() * LANE_WIDTH];

        // Reuse the lane already waiting for this commit, else open a new one.
        let lane = match active
            .iter()
            .position(|slot| slot.as_deref() == Some(commit.id.as_str()))
        {
            Some(lane) => lane,
            None => {
                let lane = active
                    .iter()
                    .position(Option::is_none)
                    .unwrap_or(active.len());
                if lane == active.len() {
                    active.push(None);
                    colors.push(0);
                }
                colors[lane] = color_counter;
                color_counter = color_counter.wrapping_add(1);
                lane
            }
        };
        if cells.len() < active.len() * LANE_WIDTH {
            cells.resize(active.len() * LANE_WIDTH, None);
        }

        // Lanes that reach this commit from above close into the node.
        let closing: Vec<usize> = active
            .iter()
            .enumerate()
            .filter(|(i, slot)| *i != lane && slot.as_deref() == Some(commit.id.as_str()))
            .map(|(i, _)| i)
            .collect();

        for (i, slot) in active.iter().enumerate() {
            if slot.is_some() && i != lane && !closing.contains(&i) {
                put(&mut cells, i * LANE_WIDTH, '│', colors[i]);
            }
        }
        for &from in &closing {
            connect(&mut cells, lane, from, colors[from], '┘', '└');
            active[from] = None;
        }

        // First parent continues in the node's lane; extra parents branch out.
        // Parents missing from this slice close the lane instead of dangling.
        let first = commit.parents.first().filter(|p| ids.contains(p.as_str()));
        active[lane] = first.cloned();
        for parent in commit.parents.iter().skip(1) {
            if !ids.contains(parent.as_str()) {
                continue;
            }
            let existing = active
                .iter()
                .position(|slot| slot.as_deref() == Some(parent.as_str()));
            match existing {
                Some(other) => {
                    // Join a lane that already leads to this parent.
                    let color = colors[other];
                    connect(&mut cells, lane, other, color, '┤', '├');
                }
                None => {
                    let target = free_lane(&active, lane);
                    if target == active.len() {
                        active.push(None);
                        colors.push(0);
                    }
                    colors[target] = color_counter;
                    color_counter = color_counter.wrapping_add(1);
                    active[target] = Some(parent.clone());
                    if cells.len() < active.len() * LANE_WIDTH {
                        cells.resize(active.len() * LANE_WIDTH, None);
                    }
                    let color = colors[target];
                    connect(&mut cells, lane, target, color, '┐', '┌');
                }
            }
        }

        put(&mut cells, lane * LANE_WIDTH, '●', colors[lane]);

        lane_count = lane_count.max(active.len());
        rows.push(GraphRow {
            cells: cells
                .into_iter()
                .map(|cell| cell.unwrap_or(GraphCell { ch: ' ', color: 0 }))
                .collect(),
        });
    }

    for row in &mut rows {
        row.cells
            .resize(lane_count * LANE_WIDTH, GraphCell { ch: ' ', color: 0 });
    }
    GraphLayout { rows, lane_count }
}

/// A free lane to the right of `after`, else any free lane, else a new lane.
fn free_lane(active: &[Option<String>], after: usize) -> usize {
    if let Some(lane) = active
        .iter()
        .enumerate()
        .skip(after + 1)
        .find_map(|(lane, slot)| slot.is_none().then_some(lane))
    {
        return lane;
    }
    active
        .iter()
        .position(Option::is_none)
        .unwrap_or(active.len())
}

fn put(cells: &mut Vec<Option<GraphCell>>, column: usize, ch: char, color: u8) {
    if column >= cells.len() {
        cells.resize(column + 1, None);
    }
    cells[column] = Some(GraphCell { ch, color });
}

/// Where a horizontal run crosses an existing lane glyph.
fn crossed(ch: char) -> char {
    match ch {
        '│' => '┼',
        '┐' | '┌' => '┬',
        '┘' | '└' => '┴',
        '├' | '┤' => '┼',
        other => other,
    }
}

/// Horizontal run plus a corner at `to_lane`.
///
/// `corner_right` is used when the target sits right of the node, `corner_left`
/// when it sits left.
fn connect(
    cells: &mut Vec<Option<GraphCell>>,
    from_lane: usize,
    to_lane: usize,
    color: u8,
    corner_right: char,
    corner_left: char,
) {
    if from_lane == to_lane {
        return;
    }
    let (corner, start, end) = if to_lane > from_lane {
        (
            corner_right,
            from_lane * LANE_WIDTH + 1,
            to_lane * LANE_WIDTH,
        )
    } else {
        (
            corner_left,
            to_lane * LANE_WIDTH + 1,
            from_lane * LANE_WIDTH,
        )
    };
    for column in start..end {
        let existing = cells.get(column).and_then(|cell| cell.as_ref());
        let ch = match existing.map(|cell| cell.ch) {
            Some(' ') | None => '─',
            Some(other) => crossed(other),
        };
        put(cells, column, ch, color);
    }
    put(cells, to_lane * LANE_WIDTH, corner, color);
}

/// Lane colors cycle through theme accents so branches stay distinguishable.
pub fn lane_color(theme: &Theme, lane: u8) -> Rgb {
    match lane % 6 {
        0 => theme.accent,
        1 => theme.git_modified,
        2 => theme.git_added,
        3 => theme.git_untracked,
        4 => theme.git_conflicted,
        _ => theme.git_deleted,
    }
}

/// One visible row inside an expanded commit's changed files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeRow {
    Folder {
        /// Repo-relative directory path (`/` separators).
        path: String,
        /// Last path segment (display name).
        name: String,
        depth: u16,
        expanded: bool,
        /// Number of changed files under this folder (recursive).
        files: usize,
    },
    /// Index into the commit's [`CommitChange`] slice.
    File { index: usize, depth: u16 },
}

/// Key for an expanded directory inside a commit.
pub fn dir_key(commit: &str, path: &str) -> String {
    format!("{commit}\u{0}{path}")
}

#[derive(Default)]
struct ChangeNode {
    dirs: BTreeMap<String, ChangeNode>,
    files: Vec<usize>,
}

impl ChangeNode {
    fn file_count(&self) -> usize {
        self.files.len()
            + self
                .dirs
                .values()
                .map(ChangeNode::file_count)
                .sum::<usize>()
    }
}

fn build_change_tree(changes: &[CommitChange]) -> ChangeNode {
    let mut root = ChangeNode::default();
    for (index, change) in changes.iter().enumerate() {
        let parts: Vec<&str> = change
            .path
            .split('/')
            .filter(|part| !part.is_empty())
            .collect();
        if parts.is_empty() {
            continue;
        }
        let mut node = &mut root;
        for segment in &parts[..parts.len() - 1] {
            node = node.dirs.entry((*segment).to_string()).or_default();
        }
        node.files.push(index);
    }
    root
}

fn flatten_change_tree(
    node: &ChangeNode,
    oid: &str,
    prefix: &str,
    depth: u16,
    expanded_dirs: &HashSet<String>,
    out: &mut Vec<ChangeRow>,
) {
    for (name, child) in &node.dirs {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let expanded = expanded_dirs.contains(&dir_key(oid, &path));
        out.push(ChangeRow::Folder {
            path: path.clone(),
            name: name.clone(),
            depth,
            expanded,
            files: child.file_count(),
        });
        if expanded {
            flatten_change_tree(
                child,
                oid,
                &path,
                depth.saturating_add(1),
                expanded_dirs,
                out,
            );
        }
    }
    for index in &node.files {
        out.push(ChangeRow::File {
            index: *index,
            depth,
        });
    }
}

/// Visible folder/file rows for one commit, starting at `depth`.
pub fn flatten_changes(
    oid: &str,
    changes: &[CommitChange],
    expanded_dirs: &HashSet<String>,
    depth: u16,
) -> Vec<ChangeRow> {
    let tree = build_change_tree(changes);
    let mut rows = Vec::with_capacity(changes.len());
    flatten_change_tree(&tree, oid, "", depth, expanded_dirs, &mut rows);
    rows
}

/// Status color for a changed file inside a commit.
pub fn change_color(theme: &Theme, kind: CommitChangeKind) -> Rgb {
    match kind {
        CommitChangeKind::Added => theme.git_added,
        CommitChangeKind::Deleted => theme.git_deleted,
        CommitChangeKind::Renamed | CommitChangeKind::Copied => theme.git_renamed,
        CommitChangeKind::TypeChanged => theme.git_conflicted,
        CommitChangeKind::Modified => theme.git_modified,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(id: &str, parents: &[&str]) -> GraphCommit {
        GraphCommit {
            id: id.repeat(40 / id.len().max(1)),
            short: id.into(),
            parents: parents
                .iter()
                .map(|parent| parent.repeat(40 / parent.len().max(1)))
                .collect(),
            refs: Vec::new(),
            author: "forge".into(),
            time: 0,
            subject: format!("commit {id}"),
        }
    }

    fn cells(layout: &GraphLayout, index: usize) -> String {
        layout
            .row(index)
            .iter()
            .map(|cell| cell.ch)
            .collect::<String>()
    }

    #[test]
    fn linear_history_is_one_lane() {
        let graph = layout_graph(&[commit("c", &["b"]), commit("b", &["a"]), commit("a", &[])]);
        assert_eq!(graph.lane_count, 1);
        for row in 0..3 {
            assert_eq!(cells(&graph, row).trim_end(), "●");
        }
    }

    #[test]
    fn merge_and_branch_use_two_lanes() {
        // m -> (a, b); a -> base; b -> base
        let graph = layout_graph(&[
            commit("m", &["a", "b"]),
            commit("a", &["base"]),
            commit("b", &["base"]),
            commit("base", &[]),
        ]);
        assert_eq!(graph.lane_count, 2);
        assert_eq!(cells(&graph, 0).trim_end(), "●─┐");
        assert_eq!(cells(&graph, 1).trim_end(), "● │");
        assert_eq!(cells(&graph, 2).trim_end(), "│ ●");
        // The second lane converges back into the base commit.
        assert_eq!(cells(&graph, 3).trim_end(), "●─┘");
    }

    #[test]
    fn octopus_merge_opens_one_lane_per_extra_parent() {
        let graph = layout_graph(&[
            commit("x", &["a", "b", "c"]),
            commit("a", &[]),
            commit("b", &[]),
            commit("c", &[]),
        ]);
        assert_eq!(graph.lane_count, 3);
        assert_eq!(
            cells(&graph, 0).trim_end(),
            "●─┬─┐",
            "{:?}",
            cells(&graph, 0)
        );
    }

    #[test]
    fn disconnected_tips_do_not_reuse_busy_lanes() {
        let graph = layout_graph(&[commit("a", &[]), commit("b", &[]), commit("c", &[])]);
        assert_eq!(graph.lane_count, 1, "free lanes are reused");
    }

    #[test]
    fn every_row_pads_to_full_lane_width() {
        let graph = layout_graph(&[commit("m", &["a", "b"]), commit("a", &[]), commit("b", &[])]);
        let width = graph.width() as usize;
        for row in 0..3 {
            assert_eq!(graph.row(row).len(), width);
        }
    }

    #[test]
    fn parents_missing_from_the_slice_close_their_lane() {
        // Filtering can drop a parent while keeping its child; the missing
        // parent must not leave a lane open (and widen the graph) below.
        let graph = layout_graph(&[commit("m", &["a", "b"]), commit("b", &[])]);
        assert_eq!(graph.lane_count, 1);
        assert_eq!(cells(&graph, 1).trim_end(), "●");
    }

    #[test]
    fn changes_flatten_into_folders_and_files() {
        let oid = "a".repeat(40);
        let changes = vec![
            CommitChange {
                path: "src/app/main.rs".into(),
                old_path: None,
                kind: CommitChangeKind::Modified,
            },
            CommitChange {
                path: "README.md".into(),
                old_path: None,
                kind: CommitChangeKind::Added,
            },
        ];
        let rows = flatten_changes(&oid, &changes, &HashSet::new(), 1);
        assert!(
            matches!(&rows[0], ChangeRow::Folder { name, files: 1, depth: 1, expanded: false, .. } if name == "src")
        );
        assert!(matches!(&rows[1], ChangeRow::File { index: 1, depth: 1 }));

        let mut dirs = HashSet::new();
        dirs.insert(dir_key(&oid, "src"));
        let rows = flatten_changes(&oid, &changes, &dirs, 1);
        assert!(matches!(&rows[1], ChangeRow::Folder { name, depth: 2, .. } if name == "app"));
        // Nested files stay hidden until their own directory is expanded.
        assert!(matches!(&rows[2], ChangeRow::File { index: 1, depth: 1 }));

        dirs.insert(dir_key(&oid, "src/app"));
        let rows = flatten_changes(&oid, &changes, &dirs, 1);
        assert!(matches!(&rows[2], ChangeRow::File { index: 0, depth: 3 }));
        assert!(matches!(&rows[3], ChangeRow::File { index: 1, depth: 1 }));
    }
}
