use super::navigation::FocusTarget;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactWorkspaceState {
    pub search: String,
    pub highlighted: Option<usize>,
    pub scroll: usize,
    pub focus: FocusTarget,
    tab: ArtifactTab,
    results: ResultsViewport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactTab {
    Summary,
    Results,
    Sql,
    Schema,
    Evidence,
}

impl ArtifactTab {
    const ALL: [Self; 5] = [
        Self::Summary,
        Self::Results,
        Self::Sql,
        Self::Schema,
        Self::Evidence,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Summary => "Summary",
            Self::Results => "Results",
            Self::Sql => "SQL",
            Self::Schema => "Schema",
            Self::Evidence => "Evidence",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultMove {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResultsMetadata {
    pub persisted_rows: usize,
    pub returned_rows: usize,
    pub columns: usize,
    pub truncated: bool,
}

impl ResultsMetadata {
    fn valid(self) -> bool {
        self.returned_rows <= self.persisted_rows
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResultsViewportPage {
    pub row_offset: usize,
    pub column_offset: usize,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResultsViewportRequest {
    pub row_offset: usize,
    pub column_offset: usize,
    pub row_count: usize,
    pub column_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultsViewport {
    metadata: ResultsMetadata,
    visible_rows: usize,
    visible_columns: usize,
    focused_row: Option<usize>,
    focused_column: Option<usize>,
    row_offset: usize,
    column_offset: usize,
    columns: Vec<String>,
    rows: Vec<Vec<String>>,
}

impl Default for ResultsViewport {
    fn default() -> Self {
        Self {
            metadata: ResultsMetadata::default(),
            visible_rows: 8,
            visible_columns: 4,
            focused_row: None,
            focused_column: None,
            row_offset: 0,
            column_offset: 0,
            columns: Vec::new(),
            rows: Vec::new(),
        }
    }
}

impl ResultsViewport {
    pub const fn metadata(&self) -> ResultsMetadata {
        self.metadata
    }

    pub const fn focus(&self) -> Option<(usize, usize)> {
        match (self.focused_row, self.focused_column) {
            (Some(row), Some(column)) => Some((row, column)),
            _ => None,
        }
    }

    pub const fn row_offset(&self) -> usize {
        self.row_offset
    }

    pub const fn column_offset(&self) -> usize {
        self.column_offset
    }

    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    pub fn rows(&self) -> &[Vec<String>] {
        &self.rows
    }

    pub const fn request(&self) -> ResultsViewportRequest {
        ResultsViewportRequest {
            row_offset: self.row_offset,
            column_offset: self.column_offset,
            row_count: self.visible_rows,
            column_count: self.visible_columns,
        }
    }

    fn resize(&mut self, rows: usize, columns: usize) -> ArtifactOutcome {
        let rows = rows.max(1);
        let columns = columns.max(1);
        if (rows, columns) == (self.visible_rows, self.visible_columns) {
            return ArtifactOutcome::Ignored;
        }
        self.visible_rows = rows;
        self.visible_columns = columns;
        self.keep_focus_visible();
        self.invalidate_page();
        if self.focus().is_some() {
            ArtifactOutcome::ViewportRequested(self.request())
        } else {
            ArtifactOutcome::Changed
        }
    }

    fn load(&mut self, metadata: ResultsMetadata, page: ResultsViewportPage) -> ArtifactOutcome {
        if !metadata.valid()
            || page.row_offset != 0
            || page.column_offset != 0
            || !page_fits(&page, self.visible_rows, self.visible_columns)
            || page.rows.len() > metadata.returned_rows
            || page.columns.len() > metadata.columns
        {
            return ArtifactOutcome::Ignored;
        }
        self.metadata = metadata;
        self.focused_row = (metadata.returned_rows > 0 && metadata.columns > 0).then_some(0);
        self.focused_column = self.focused_row.map(|_| 0);
        self.row_offset = 0;
        self.column_offset = 0;
        self.columns = page.columns;
        self.rows = page.rows;
        ArtifactOutcome::Changed
    }

    fn apply_page(&mut self, page: ResultsViewportPage) -> ArtifactOutcome {
        if page.row_offset != self.row_offset
            || page.column_offset != self.column_offset
            || !page_fits(&page, self.visible_rows, self.visible_columns)
            || page.row_offset.saturating_add(page.rows.len()) > self.metadata.returned_rows
            || page.column_offset.saturating_add(page.columns.len()) > self.metadata.columns
        {
            return ArtifactOutcome::Ignored;
        }
        self.columns = page.columns;
        self.rows = page.rows;
        ArtifactOutcome::Changed
    }

    fn move_focus(&mut self, direction: ResultMove) -> ArtifactOutcome {
        let Some((row, column)) = self.focus() else {
            return ArtifactOutcome::Ignored;
        };
        let (next_row, next_column) = match direction {
            ResultMove::Up => (row.saturating_sub(1), column),
            ResultMove::Down => (
                row.saturating_add(1)
                    .min(self.metadata.returned_rows.saturating_sub(1)),
                column,
            ),
            ResultMove::Left => (row, column.saturating_sub(1)),
            ResultMove::Right => (
                row,
                column
                    .saturating_add(1)
                    .min(self.metadata.columns.saturating_sub(1)),
            ),
        };
        if (next_row, next_column) == (row, column) {
            return ArtifactOutcome::Ignored;
        }
        self.focused_row = Some(next_row);
        self.focused_column = Some(next_column);
        if self.keep_focus_visible() {
            self.invalidate_page();
            ArtifactOutcome::ViewportRequested(self.request())
        } else {
            ArtifactOutcome::Changed
        }
    }

    fn keep_focus_visible(&mut self) -> bool {
        let Some((row, column)) = self.focus() else {
            return false;
        };
        let previous = (self.row_offset, self.column_offset);
        if row < self.row_offset {
            self.row_offset = row;
        } else if row >= self.row_offset.saturating_add(self.visible_rows) {
            self.row_offset = row + 1 - self.visible_rows;
        }
        if column < self.column_offset {
            self.column_offset = column;
        } else if column >= self.column_offset.saturating_add(self.visible_columns) {
            self.column_offset = column + 1 - self.visible_columns;
        }
        previous != (self.row_offset, self.column_offset)
    }

    fn invalidate_page(&mut self) {
        self.columns.clear();
        self.rows.clear();
    }
}

fn page_fits(page: &ResultsViewportPage, visible_rows: usize, visible_columns: usize) -> bool {
    page.rows.len() <= visible_rows
        && page.columns.len() <= visible_columns
        && page.rows.iter().all(|row| row.len() == page.columns.len())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactAction {
    NextTab,
    PreviousTab,
    ResizeResults {
        visible_rows: usize,
        visible_columns: usize,
    },
    ResultsLoaded {
        metadata: ResultsMetadata,
        page: ResultsViewportPage,
    },
    ViewportLoaded(ResultsViewportPage),
    MoveResult(ResultMove),
    Back,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactOutcome {
    Changed,
    ViewportRequested(ResultsViewportRequest),
    Ignored,
    Close,
}

impl Default for ArtifactWorkspaceState {
    fn default() -> Self {
        Self {
            search: String::new(),
            highlighted: None,
            scroll: 0,
            focus: FocusTarget::ArtifactContent,
            tab: ArtifactTab::Results,
            results: ResultsViewport::default(),
        }
    }
}

impl ArtifactWorkspaceState {
    pub const fn tab(&self) -> ArtifactTab {
        self.tab
    }

    pub const fn results(&self) -> &ResultsViewport {
        &self.results
    }

    pub fn reduce(&mut self, action: ArtifactAction) -> ArtifactOutcome {
        match action {
            ArtifactAction::NextTab => {
                self.tab = adjacent_tab(self.tab, 1);
                ArtifactOutcome::Changed
            }
            ArtifactAction::PreviousTab => {
                self.tab = adjacent_tab(self.tab, ArtifactTab::ALL.len() - 1);
                ArtifactOutcome::Changed
            }
            ArtifactAction::ResizeResults {
                visible_rows,
                visible_columns,
            } => self.results.resize(visible_rows, visible_columns),
            ArtifactAction::ResultsLoaded { metadata, page } => self.results.load(metadata, page),
            ArtifactAction::ViewportLoaded(page) => self.results.apply_page(page),
            ArtifactAction::MoveResult(direction) => {
                if self.tab != ArtifactTab::Results {
                    ArtifactOutcome::Ignored
                } else {
                    self.results.move_focus(direction)
                }
            }
            ArtifactAction::Back => ArtifactOutcome::Close,
        }
    }
}

fn adjacent_tab(current: ArtifactTab, offset: usize) -> ArtifactTab {
    let index = ArtifactTab::ALL
        .iter()
        .position(|tab| *tab == current)
        .unwrap_or(1);
    ArtifactTab::ALL[(index + offset) % ArtifactTab::ALL.len()]
}

pub fn render_lines(state: &ArtifactWorkspaceState) -> Vec<String> {
    let mut lines = vec!["Artifact".to_owned()];
    lines.push(
        ArtifactTab::ALL
            .iter()
            .map(|tab| {
                if *tab == state.tab {
                    format!("[{}]", tab.label())
                } else {
                    tab.label().to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("  "),
    );
    if !state.search.is_empty() {
        lines.push(format!("Search · {}", state.search));
    }
    if state.tab == ArtifactTab::Results {
        let metadata = state.results.metadata();
        lines.push(format!(
            "Rows · {} returned / {} persisted{}",
            metadata.returned_rows,
            metadata.persisted_rows,
            if metadata.truncated {
                " · preview limited"
            } else {
                ""
            }
        ));
        if !state.results.columns().is_empty() {
            lines.push(state.results.columns().join(" | "));
            lines.extend(state.results.rows().iter().map(|row| row.join(" | ")));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first_page() -> ResultsViewportPage {
        ResultsViewportPage {
            row_offset: 0,
            column_offset: 0,
            columns: vec!["order_id".to_owned(), "amount".to_owned()],
            rows: vec![
                vec!["A-1".to_owned(), "12.00".to_owned()],
                vec!["A-2".to_owned(), "18.00".to_owned()],
                vec!["A-3".to_owned(), "23.00".to_owned()],
            ],
        }
    }

    #[test]
    fn artifact_tabs_default_to_results_and_wrap_in_both_directions() {
        let mut state = ArtifactWorkspaceState::default();
        assert_eq!(state.tab(), ArtifactTab::Results);
        for expected in [
            ArtifactTab::Sql,
            ArtifactTab::Schema,
            ArtifactTab::Evidence,
            ArtifactTab::Summary,
            ArtifactTab::Results,
        ] {
            assert_eq!(
                state.reduce(ArtifactAction::NextTab),
                ArtifactOutcome::Changed
            );
            assert_eq!(state.tab(), expected);
        }
        assert_eq!(
            state.reduce(ArtifactAction::PreviousTab),
            ArtifactOutcome::Changed
        );
        assert_eq!(state.tab(), ArtifactTab::Summary);
    }

    #[test]
    fn result_focus_and_offsets_are_clamped_and_request_only_the_current_viewport() {
        let mut state = ArtifactWorkspaceState::default();
        state.reduce(ArtifactAction::ResizeResults {
            visible_rows: 3,
            visible_columns: 2,
        });
        assert_eq!(
            state.reduce(ArtifactAction::ResultsLoaded {
                metadata: ResultsMetadata {
                    persisted_rows: 125,
                    returned_rows: 100,
                    columns: 20,
                    truncated: true,
                },
                page: first_page(),
            }),
            ArtifactOutcome::Changed
        );
        assert_eq!(state.results().focus(), Some((0, 0)));
        assert_eq!(state.results().rows().len(), 3);
        assert_eq!(state.results().columns().len(), 2);

        for _ in 0..150 {
            state.reduce(ArtifactAction::MoveResult(ResultMove::Down));
        }
        for _ in 0..30 {
            state.reduce(ArtifactAction::MoveResult(ResultMove::Right));
        }
        let results = state.results();
        assert_eq!(results.focus(), Some((99, 19)));
        assert_eq!(results.row_offset(), 97);
        assert_eq!(results.column_offset(), 18);
        assert!(
            results.rows().is_empty(),
            "old viewport cells are discarded"
        );
        assert!(results.columns().is_empty());

        let request = state.results().request();
        assert_eq!(request.row_offset, 97);
        assert_eq!(request.column_offset, 18);
        assert_eq!(request.row_count, 3);
        assert_eq!(request.column_count, 2);
    }

    #[test]
    fn stale_or_oversized_pages_cannot_replace_the_current_window() {
        let mut state = ArtifactWorkspaceState::default();
        state.reduce(ArtifactAction::ResizeResults {
            visible_rows: 2,
            visible_columns: 2,
        });
        state.reduce(ArtifactAction::ResultsLoaded {
            metadata: ResultsMetadata {
                persisted_rows: 4,
                returned_rows: 4,
                columns: 3,
                truncated: false,
            },
            page: ResultsViewportPage {
                row_offset: 0,
                column_offset: 0,
                columns: vec!["a".to_owned(), "b".to_owned()],
                rows: vec![vec!["1".to_owned(), "2".to_owned()]],
            },
        });
        state.reduce(ArtifactAction::MoveResult(ResultMove::Down));
        state.reduce(ArtifactAction::MoveResult(ResultMove::Down));
        assert_eq!(state.results().row_offset(), 1);

        assert_eq!(
            state.reduce(ArtifactAction::ViewportLoaded(ResultsViewportPage {
                row_offset: 0,
                column_offset: 0,
                columns: vec!["stale".to_owned()],
                rows: vec![vec!["stale".to_owned()]],
            })),
            ArtifactOutcome::Ignored
        );
        assert_eq!(
            state.reduce(ArtifactAction::ViewportLoaded(ResultsViewportPage {
                row_offset: 1,
                column_offset: 0,
                columns: vec!["a".to_owned(), "b".to_owned(), "too-many".to_owned()],
                rows: vec![vec!["3".to_owned(), "4".to_owned()]],
            })),
            ArtifactOutcome::Ignored
        );
        assert!(state.results().rows().is_empty());
    }

    #[test]
    fn empty_results_have_no_fake_focus_and_navigation_has_no_execution_intent() {
        let mut state = ArtifactWorkspaceState::default();
        state.reduce(ArtifactAction::ResultsLoaded {
            metadata: ResultsMetadata {
                persisted_rows: 0,
                returned_rows: 0,
                columns: 2,
                truncated: false,
            },
            page: ResultsViewportPage::default(),
        });
        assert_eq!(state.results().focus(), None);
        assert_eq!(
            state.reduce(ArtifactAction::MoveResult(ResultMove::Down)),
            ArtifactOutcome::Ignored
        );
        assert_eq!(state.reduce(ArtifactAction::Back), ArtifactOutcome::Close);
    }

    #[test]
    fn resizing_a_loaded_result_requests_exactly_one_new_bounded_page() {
        let mut state = ArtifactWorkspaceState::default();
        state.reduce(ArtifactAction::ResultsLoaded {
            metadata: ResultsMetadata {
                persisted_rows: 3,
                returned_rows: 3,
                columns: 2,
                truncated: false,
            },
            page: first_page(),
        });
        assert_eq!(
            state.reduce(ArtifactAction::ResizeResults {
                visible_rows: 2,
                visible_columns: 1,
            }),
            ArtifactOutcome::ViewportRequested(ResultsViewportRequest {
                row_offset: 0,
                column_offset: 0,
                row_count: 2,
                column_count: 1,
            })
        );
        assert!(state.results().rows().is_empty());
        assert_eq!(
            state.reduce(ArtifactAction::ResizeResults {
                visible_rows: 2,
                visible_columns: 1,
            }),
            ArtifactOutcome::Ignored
        );
    }
}
