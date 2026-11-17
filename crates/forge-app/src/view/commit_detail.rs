//! Theme-aware read-only Git History commit details.
use super::*;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Default)]
struct Detail {
    sha: String,
    author: String,
    authored: String,
    committer: String,
    committed: String,
    message: Vec<String>,
    files: Vec<(String, usize, usize, bool)>,
    summary: String,
}

fn parse(lines: &[String]) -> Detail {
    let mut d = Detail::default();
    let mut message = false;
    let mut files = false;
    for line in lines {
        let t = line.trim();
        if let Some(v) = line.strip_prefix("commit ") {
            d.sha = v.trim().into();
        } else if let Some(v) = line.strip_prefix("Author:") {
            d.author = v.trim().into();
        } else if let Some(v) = line.strip_prefix("AuthorDate:") {
            d.authored = v.trim().into();
        } else if let Some(v) = line.strip_prefix("Commit:") {
            d.committer = v.trim().into();
        } else if let Some(v) = line.strip_prefix("CommitDate:") {
            d.committed = v.trim().into();
        } else if !files && line.starts_with("    ") && !t.is_empty() {
            message = true;
            d.message.push(t.into());
        } else if t.is_empty() {
            if message {
                files = true;
            }
        } else if files && t.contains(" | ") {
            if let Some((path, stat)) = t.rsplit_once(" | ") {
                d.files.push((
                    path.trim().into(),
                    stat.matches('+').count(),
                    stat.matches('-').count(),
                    stat.contains("Bin ") || stat.contains("binary"),
                ));
            }
        } else if files && t.contains("file") && t.contains(" changed") {
            d.summary = t.into();
        }
    }
    d
}
fn clip(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.into();
    }
    let mut s = String::new();
    let mut left = width.saturating_sub(1);
    for c in text.chars() {
        let n = c.width().unwrap_or(0);
        if n > left {
            break;
        }
        left -= n;
        s.push(c);
    }
    if width > 0 {
        s.push('…');
    }
    s
}
fn field(name: &'static str, value: &str, width: usize, p: Palette) -> Line<'static> {
    let label = 12usize.min(width);
    Line::from(vec![
        Span::styled(format!("{name:<label$}"), Style::default().fg(p.fg_dim)),
        Span::styled(
            clip(value, width.saturating_sub(label)),
            Style::default().fg(p.fg),
        ),
    ])
}
pub(crate) fn draw(frame: &mut Frame<'_>, area: Rect, model: &mut App, p: Palette) -> bool {
    // History folders only navigate the file tree; they do not replace the
    // currently loaded commit preview. Treat them as the owning commit so the
    // view does not fall back to raw git-show text with source line numbers.
    let index = match model.selected_scm_row() {
        Some(ScmRow::Commit { index }) => index,
        Some(ScmRow::CommitFolder { commit, .. }) => commit,
        _ => return false,
    };
    let Some(commit) = model.history.commits.get(index) else {
        return false;
    };
    if !model.in_git_sidebar() || model.preview_state.preview_loading {
        return false;
    }
    let Some(Preview {
        path,
        content: PreviewContent::Text { lines, .. },
    }) = &model.preview_state.preview
    else {
        return false;
    };
    if path.to_string_lossy() != commit.id {
        return false;
    }
    let d = parse(lines);
    if d.sha.is_empty() {
        return false;
    }
    frame.render_widget(Block::default().style(Style::default().bg(p.bg)), area);
    let width = area.width as usize;
    let content_width = width.saturating_sub(4);
    let mut rows: Vec<Line<'static>> = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "  ● COMMIT  ",
                Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                clip(&d.sha, content_width.saturating_sub(10)),
                Style::default().fg(p.fg_dim),
            ),
        ]),
        Line::from(""),
    ];
    for (i, text) in d.message.iter().enumerate() {
        rows.push(Line::from(Span::styled(
            format!("  {}", clip(text, content_width)),
            Style::default()
                .fg(if i == 0 { p.fg } else { p.fg_dim })
                .add_modifier(if i == 0 {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        )));
    }
    rows.push(Line::from(""));
    rows.push(field("  Author", &d.author, width, p));
    rows.push(field("  Authored", &d.authored, width, p));
    if d.committer != d.author && !d.committer.is_empty() {
        rows.push(field("  Committer", &d.committer, width, p));
    }
    rows.push(field("  Committed", &d.committed, width, p));
    rows.push(Line::from(""));
    rows.push(Line::from(vec![
        Span::styled(
            "  CHANGED FILES",
            Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  ({})", d.files.len()),
            Style::default().fg(p.fg_dim),
        ),
    ]));
    rows.push(Line::from(Span::styled(
        format!("  {}", "─".repeat(content_width.min(90))),
        Style::default().fg(p.border),
    )));
    for (path, plus, minus, binary) in &d.files {
        let suffix_width = 18usize.min(content_width / 2);
        let path_width = content_width.saturating_sub(suffix_width);
        let path = clip(path, path_width);
        let padding = path_width.saturating_sub(path.width());
        let mut spans = vec![
            Span::styled("  ", Style::default()),
            Span::styled(path, Style::default().fg(p.fg)),
            Span::raw(" ".repeat(padding)),
        ];
        if *binary {
            spans.push(Span::styled(" binary", Style::default().fg(p.warning)));
        } else {
            spans.push(Span::styled(
                format!(" +{plus}"),
                Style::default().fg(p.success),
            ));
            spans.push(Span::styled(
                format!("  -{minus}"),
                Style::default().fg(p.error),
            ));
        }
        rows.push(Line::from(spans));
    }
    rows.push(Line::from(""));
    if !d.summary.is_empty() {
        rows.push(Line::from(Span::styled(
            format!("  {}", clip(&d.summary, content_width)),
            Style::default().fg(p.info),
        )));
    }
    let maximum = rows.len().saturating_sub(area.height as usize);
    model.preview_state.preview_scroll = model.preview_state.preview_scroll.min(maximum);
    frame.render_widget(
        Paragraph::new(rows)
            .scroll((
                model.preview_state.preview_scroll.min(u16::MAX as usize) as u16,
                0,
            ))
            .style(Style::default().bg(p.bg)),
        area,
    );
    model.session.layout.preview_inner = area;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_git_show_and_separates_additions_from_deletions() {
        let text = r#"commit 5f68c69da6094844095679dc1359f711f82c51e9
Author:     Test User <test@example.com>
AuthorDate: Tue Nov 17 11:21:15 2026 +0800
Commit:     Test User <test@example.com>
CommitDate: Tue Nov 17 11:21:15 2026 +0800

    fix(site): align v0.3.2 landing metadata with release version

 web/src/components/Landing.astro      | 10 +++++-----
 web/src/layouts/Base.astro            |  4 ++--
 web/src/pages/en/releases/index.astro |  2 +-
 web/src/pages/releases/index.astro    |  2 +-
 4 files changed, 9 insertions(+), 9 deletions(-)"#;
        let rows = text.lines().map(str::to_owned).collect::<Vec<_>>();
        let d = parse(&rows);
        assert!(d.sha.starts_with("5f68c69"));
        assert_eq!(d.author, "Test User <test@example.com>");
        assert_eq!(d.authored, "Tue Nov 17 11:21:15 2026 +0800");
        assert_eq!(
            d.message,
            vec!["fix(site): align v0.3.2 landing metadata with release version"]
        );
        assert_eq!(d.files.len(), 4);
        assert_eq!((d.files[0].1, d.files[0].2), (5, 5));
        assert_eq!((d.files[1].1, d.files[1].2), (2, 2));
        assert_eq!(d.files.iter().map(|f| f.1).sum::<usize>(), 9);
        assert_eq!(d.files.iter().map(|f| f.2).sum::<usize>(), 9);
        assert!(d.summary.contains("4 files changed"));
    }

    #[test]
    fn multibyte_paths_clip_to_display_cell_width() {
        assert_eq!(clip("中文abc", 5), "中文…");
        assert_eq!(clip("forge", 5), "forge");
        assert_eq!(clip("anything", 0), "");
    }

    #[test]
    fn multiline_commit_message_and_binary_stat() {
        let lines = [
            "commit aabb",
            "Author: Jane",
            "",
            "    Subject",
            "    Body line 1",
            "    Body line 2",
            "",
            " assets/logo.png | Bin 0 -> 221 bytes",
            " 1 file changed",
        ]
        .map(str::to_owned)
        .to_vec();
        let detail = parse(&lines);
        assert_eq!(
            detail.message,
            vec!["Subject", "Body line 1", "Body line 2"]
        );
        assert_eq!(detail.files.len(), 1);
        assert!(detail.files[0].3);
    }
}
