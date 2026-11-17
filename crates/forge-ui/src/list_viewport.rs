//! Shared viewport calculation for Files and Git sidebars.
//!
//! Keeps scrolling, selection-following and sticky ancestor positioning in one
//! place while leaving row creation and item-specific behavior to each view.
use crate::sticky;

/// Keep the selected item in view without affecting the row identity.
/// Also available to legacy callers through `forge_ui::ensure_selected_visible`.
pub fn ensure_selected_visible(selected: usize, scroll: &mut usize, height: usize, len: usize) {
    if len == 0 || height == 0 {
        *scroll = 0;
        return;
    }
    let max_scroll = len.saturating_sub(height);
    if *scroll > max_scroll {
        *scroll = max_scroll;
    }
    if selected < *scroll {
        *scroll = selected;
    } else if selected >= *scroll + height {
        *scroll = selected + 1 - height;
    }
    if *scroll > max_scroll {
        *scroll = max_scroll;
    }
}

/// The visible original-row range and any pinned ancestor rows.
///
/// The caller draws `pinned` first, then `visible`; neither operation changes
/// the underlying row indices used for selection and Git actions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListViewport {
    pub pinned: Vec<usize>,
    pub visible: std::ops::Range<usize>,
}

impl ListViewport {
    pub fn calculate(
        len: usize,
        selected: usize,
        scroll: &mut usize,
        height: usize,
        sticky_scroll: bool,
        follow_selection: bool,
        parent: impl Fn(usize) -> Option<usize> + Copy,
    ) -> Self {
        let pinned = if sticky_scroll {
            sticky::layout(len, selected, scroll, height, parent, follow_selection)
        } else {
            if follow_selection {
                ensure_selected_visible(selected, scroll, height, len);
            } else {
                *scroll = (*scroll).min(len.saturating_sub(height));
            }
            Vec::new()
        };
        let start = *scroll;
        let end = start
            .saturating_add(height.saturating_sub(pinned.len()))
            .min(len);
        Self {
            pinned,
            visible: start..end,
        }
    }

    /// Draw order, preserving original indices even with pinned ancestors.
    pub fn indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.pinned.iter().copied().chain(self.visible.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::ListViewport;

    // Hierarchy: 0=root, 1=folder, 2.. are children.
    fn parent(i: usize) -> Option<usize> {
        match i {
            0 => None,
            1 => Some(0),
            _ => Some(1),
        }
    }

    #[test]
    fn normal_list_follows_selection_and_clamps_manual_scroll() {
        let mut scroll = 0;
        let view = ListViewport::calculate(50, 15, &mut scroll, 10, false, true, parent);
        assert_eq!(scroll, 6);
        assert_eq!(view.visible, 6..16);
        assert!(view.pinned.is_empty());

        scroll = 100;
        let view = ListViewport::calculate(50, 0, &mut scroll, 10, false, false, parent);
        assert_eq!(scroll, 40);
        assert_eq!(view.visible, 40..50);
    }

    #[test]
    fn sticky_list_preserves_ancestor_indices_and_viewport_bounds() {
        let mut scroll = 8;
        let view = ListViewport::calculate(30, 8, &mut scroll, 8, true, false, parent);
        assert_eq!(view.pinned, vec![0, 1]);
        assert_eq!(view.indices().collect::<Vec<_>>()[..2], [0, 1]);
        assert_eq!(view.visible.start, scroll);
        assert_eq!(view.indices().count(), 8);
    }

    #[test]
    fn empty_and_zero_height_have_no_visible_rows() {
        let mut scroll = 5;
        let view = ListViewport::calculate(0, 0, &mut scroll, 10, true, false, parent);
        assert_eq!(scroll, 0);
        assert!(view.visible.is_empty());
        assert!(view.pinned.is_empty());
        let view = ListViewport::calculate(20, 10, &mut scroll, 0, true, true, parent);
        assert_eq!(scroll, 0);
        assert!(view.visible.is_empty());
    }
}
