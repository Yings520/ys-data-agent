use serde_json::Value;
use ys_agent_runtime::{QueryArtifact, QueryResultPreviewView};

use super::navigation::FocusTarget;

pub const MAX_CELL_CHARS: usize = 256;
const MAX_TAB_LINES: usize = 32;
const MAX_LINE_CHARS: usize = 240;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactWorkspaceState {
    pub search: String,
    pub highlighted: Option<usize>,
    pub scroll: usize,
    pub focus: FocusTarget,
    tab: ArtifactTab,
    results: ResultsViewport,
    projection: Option<ArtifactWorkspaceProjection>,
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

#[derive(Debug, Clone, Copy)]
pub struct AuthorizedResultsPreview<'a> {
    columns: &'a [String],
    rows: &'a [Vec<Value>],
    persisted_rows: usize,
    returned_rows: usize,
    truncated: bool,
}

impl<'a> AuthorizedResultsPreview<'a> {
    pub fn new(
        columns: &'a [String],
        rows: &'a [Vec<Value>],
        persisted_rows: usize,
        returned_rows: usize,
        truncated: bool,
    ) -> Self {
        Self {
            columns,
            rows,
            persisted_rows,
            returned_rows: returned_rows.min(rows.len()),
            truncated,
        }
    }
}

impl<'a> From<&'a QueryResultPreviewView> for AuthorizedResultsPreview<'a> {
    fn from(preview: &'a QueryResultPreviewView) -> Self {
        Self::new(
            preview.columns(),
            preview.rows(),
            preview.persisted_row_count(),
            preview.returned_row_count(),
            preview.truncated(),
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ResultsAccess<'a> {
    Available(AuthorizedResultsPreview<'a>),
    Missing,
    PolicyRestricted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactUnavailableReason {
    Missing,
    PolicyRestricted,
    StatusUnavailable,
}

impl ArtifactUnavailableReason {
    const fn message(self) -> &'static str {
        match self {
            Self::Missing => "Artifact is missing",
            Self::PolicyRestricted => "Artifact is restricted by Policy",
            Self::StatusUnavailable => "Artifact status unavailable · retry from Timeline",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProjectedResults {
    Available {
        metadata: ResultsMetadata,
        page: Option<ResultsViewportPage>,
        request: ResultsViewportRequest,
    },
    Missing,
    PolicyRestricted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactWorkspaceProjection {
    unavailable: Option<ArtifactUnavailableReason>,
    summary: Vec<String>,
    sql: Vec<String>,
    schema: Vec<String>,
    evidence: Vec<String>,
    results: ProjectedResults,
}

impl ArtifactWorkspaceProjection {
    pub fn authorized(
        artifact: &QueryArtifact,
        source_display_name: &str,
        results: ResultsAccess<'_>,
        request: ResultsViewportRequest,
    ) -> Self {
        let verification = if artifact.verification.hard_failures.is_empty() {
            "Verified"
        } else {
            "Verification failed"
        };
        let mut summary = vec![
            format!("Answer · {}", safe_line(&artifact.answer_summary)),
            format!("Intent · {:?}", artifact.intent),
            format!("Semantic status · {:?}", artifact.semantic_status),
            format!("Source · {}", safe_line(source_display_name)),
            format!("Sensitivity · {:?}", artifact.sensitivity),
            format!("Verification · {verification}"),
        ];
        summary.extend(
            artifact
                .warning_codes
                .iter()
                .take(MAX_TAB_LINES.saturating_sub(summary.len()))
                .map(|warning| format!("Query warning · {}", safe_line(warning))),
        );

        let mut sql = vec!["view only · no rerun".to_owned()];
        match artifact.executed_sql.as_deref() {
            Some(executed) => sql.extend(safe_multiline(executed)),
            None => sql.push("No SQL was executed for this Artifact".to_owned()),
        }
        sql.extend(
            artifact
                .bound_parameters
                .iter()
                .take(MAX_TAB_LINES.saturating_sub(sql.len()))
                .map(|parameter| {
                    format!(
                        "Parameter · {:?} · {}",
                        parameter.kind,
                        safe_line(&parameter.display)
                    )
                }),
        );

        let mut schema = artifact
            .result_schema
            .columns
            .iter()
            .take(MAX_TAB_LINES.saturating_sub(2))
            .map(|column| {
                format!(
                    "{} · {}",
                    safe_line(&column.name),
                    column
                        .data_type
                        .as_deref()
                        .map(safe_line)
                        .unwrap_or_else(|| "type unavailable".to_owned())
                )
            })
            .collect::<Vec<_>>();
        schema.push(format!("Semantic status · {:?}", artifact.semantic_status));
        schema.extend(
            artifact
                .source_relations
                .iter()
                .take(MAX_TAB_LINES.saturating_sub(schema.len()))
                .map(|source| format!("Allowed source · {}", safe_line(source))),
        );

        let mut evidence = artifact
            .verification
            .checks
            .iter()
            .take(MAX_TAB_LINES)
            .map(|check| {
                format!(
                    "{} · {}",
                    safe_line(&check.code),
                    if check.passed { "passed" } else { "failed" }
                )
            })
            .collect::<Vec<_>>();
        evidence.extend(
            artifact
                .verification
                .evidence_refs
                .iter()
                .take(MAX_TAB_LINES.saturating_sub(evidence.len()))
                .map(|reference| format!("{:?} evidence · available", reference.metadata.kind)),
        );
        if evidence.is_empty() {
            evidence.push("No separate Evidence Artifact was recorded".to_owned());
        }

        let results = match results {
            ResultsAccess::Available(preview) => ProjectedResults::Available {
                metadata: ResultsMetadata {
                    persisted_rows: preview.persisted_rows.max(preview.returned_rows),
                    returned_rows: preview.returned_rows,
                    columns: preview.columns.len(),
                    truncated: preview.truncated,
                },
                page: Some(project_results_viewport(preview, request)),
                request,
            },
            ResultsAccess::Missing => ProjectedResults::Missing,
            ResultsAccess::PolicyRestricted => ProjectedResults::PolicyRestricted,
        };

        Self {
            unavailable: None,
            summary,
            sql,
            schema,
            evidence,
            results,
        }
    }

    pub fn unavailable(reason: ArtifactUnavailableReason) -> Self {
        Self {
            unavailable: Some(reason),
            summary: Vec::new(),
            sql: Vec::new(),
            schema: Vec::new(),
            evidence: Vec::new(),
            results: match reason {
                ArtifactUnavailableReason::Missing => ProjectedResults::Missing,
                ArtifactUnavailableReason::PolicyRestricted => ProjectedResults::PolicyRestricted,
                ArtifactUnavailableReason::StatusUnavailable => ProjectedResults::Missing,
            },
        }
    }
}

pub fn project_results_viewport(
    preview: AuthorizedResultsPreview<'_>,
    request: ResultsViewportRequest,
) -> ResultsViewportPage {
    let columns = preview
        .columns
        .iter()
        .skip(request.column_offset)
        .take(request.column_count)
        .map(|column| safe_cell(column))
        .collect::<Vec<_>>();
    let rows = preview
        .rows
        .iter()
        .take(preview.returned_rows)
        .skip(request.row_offset)
        .take(request.row_count)
        .map(|row| {
            row.iter()
                .skip(request.column_offset)
                .take(columns.len())
                .map(render_cell)
                .collect::<Vec<_>>()
        })
        .collect();
    ResultsViewportPage {
        row_offset: request.row_offset,
        column_offset: request.column_offset,
        columns,
        rows,
    }
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

    fn load_projection(
        &mut self,
        metadata: ResultsMetadata,
        page: ResultsViewportPage,
        request: ResultsViewportRequest,
    ) -> ArtifactOutcome {
        if request.row_offset != 0 || request.column_offset != 0 {
            return ArtifactOutcome::Ignored;
        }
        self.visible_rows = request.row_count.max(1);
        self.visible_columns = request.column_count.max(1);
        self.load(metadata, page)
    }

    fn clear(&mut self) {
        let visible_rows = self.visible_rows;
        let visible_columns = self.visible_columns;
        *self = Self {
            visible_rows,
            visible_columns,
            ..Self::default()
        };
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
    ProjectionLoaded(ArtifactWorkspaceProjection),
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
            projection: None,
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
            ArtifactAction::ProjectionLoaded(mut projection) => {
                let outcome = match &mut projection.results {
                    ProjectedResults::Available {
                        metadata,
                        page,
                        request,
                    } => page.take().map_or(ArtifactOutcome::Ignored, |page| {
                        self.results.load_projection(*metadata, page, *request)
                    }),
                    ProjectedResults::Missing | ProjectedResults::PolicyRestricted => {
                        self.results.clear();
                        ArtifactOutcome::Changed
                    }
                };
                if outcome != ArtifactOutcome::Ignored {
                    self.projection = Some(projection);
                }
                outcome
            }
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
    if let Some(projection) = state.projection.as_ref() {
        return render_projected_lines(state, projection);
    }
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

pub fn render_projected_lines(
    state: &ArtifactWorkspaceState,
    projection: &ArtifactWorkspaceProjection,
) -> Vec<String> {
    let mut lines = vec!["Artifact".to_owned(), render_tabs(state.tab)];
    if let Some(reason) = projection.unavailable {
        lines.push(reason.message().to_owned());
        return lines;
    }
    match state.tab {
        ArtifactTab::Summary => lines.extend(projection.summary.iter().cloned()),
        ArtifactTab::Sql => lines.extend(projection.sql.iter().cloned()),
        ArtifactTab::Schema => lines.extend(projection.schema.iter().cloned()),
        ArtifactTab::Evidence => lines.extend(projection.evidence.iter().cloned()),
        ArtifactTab::Results => match &projection.results {
            ProjectedResults::Missing => lines.push("Result Artifact is missing".to_owned()),
            ProjectedResults::PolicyRestricted => {
                lines.push("Results are restricted by Policy".to_owned())
            }
            ProjectedResults::Available { metadata, .. } => {
                lines.push(format!(
                    "Rows · {} returned / {} persisted",
                    metadata.returned_rows, metadata.persisted_rows
                ));
                if metadata.truncated {
                    lines.push(
                        "UI preview limited · the persisted Query result is unchanged".to_owned(),
                    );
                }
                if !state.results.columns().is_empty() {
                    lines.push(state.results.columns().join(" | "));
                    lines.extend(state.results.rows().iter().map(|row| row.join(" | ")));
                }
            }
        },
    }
    lines
}

fn render_tabs(active: ArtifactTab) -> String {
    ArtifactTab::ALL
        .iter()
        .map(|tab| {
            if *tab == active {
                format!("[{}]", tab.label())
            } else {
                tab.label().to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("  ")
}

fn render_cell(value: &Value) -> String {
    match value {
        Value::String(value) => safe_cell(value),
        value => safe_cell(&value.to_string()),
    }
}

fn safe_cell(value: &str) -> String {
    safe_text(value, MAX_CELL_CHARS)
}

fn safe_line(value: &str) -> String {
    safe_text(value, MAX_LINE_CHARS)
}

fn safe_multiline(value: &str) -> Vec<String> {
    value
        .lines()
        .take(MAX_TAB_LINES)
        .map(safe_line)
        .filter(|line| !line.is_empty())
        .collect()
}

fn safe_text(value: &str, max_chars: usize) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .take(max_chars)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use serde_json::{Value, json};
    use ys_agent_core::{QueryIntent, RetentionPolicy, SemanticStatus, Sensitivity, SourceId};
    use ys_agent_runtime::{QueryArtifact, VerificationCheck, VerificationReport};

    use super::*;

    struct OwnedAuthorizedResultsPreview {
        columns: Vec<String>,
        rows: Vec<Vec<Value>>,
        persisted_rows: usize,
        truncated: bool,
    }

    impl OwnedAuthorizedResultsPreview {
        fn as_view(&self) -> AuthorizedResultsPreview<'_> {
            AuthorizedResultsPreview::new(
                &self.columns,
                &self.rows,
                self.persisted_rows,
                self.rows.len(),
                self.truncated,
            )
        }
    }

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

    fn artifact() -> QueryArtifact {
        QueryArtifact {
            question: "How many governed orders?".to_owned(),
            intent: QueryIntent::AdHocRead,
            answer_summary: "There are 42 governed orders.".to_owned(),
            metric: None,
            semantic_status: SemanticStatus::Inferred,
            source_id: SourceId::new("internal-source-id"),
            source_relations: vec!["analytics.orders".to_owned()],
            time_range: None,
            executed_sql: Some("SELECT count(*)\nFROM analytics.orders".to_owned()),
            bound_parameters: Vec::new(),
            result_schema: Default::default(),
            result_artifact: None,
            freshness: None,
            verification: VerificationReport {
                checks: vec![VerificationCheck {
                    code: "policy_scope".to_owned(),
                    passed: true,
                    detail: "raw check detail must not render".to_owned(),
                }],
                hard_failures: Vec::new(),
                warnings: vec!["query_result_truncated".to_owned()],
                evidence_refs: Vec::new(),
            },
            assumptions: vec!["raw assumption must not render".to_owned()],
            warning_codes: vec!["query_result_truncated".to_owned()],
            sensitivity: Sensitivity::Internal,
            retention_policy: RetentionPolicy::Session,
            expires_at: None,
            generated_at: Utc::now(),
        }
    }

    fn preview() -> OwnedAuthorizedResultsPreview {
        OwnedAuthorizedResultsPreview {
            columns: vec![
                "order_id".to_owned(),
                "amount".to_owned(),
                "outside_column".to_owned(),
            ],
            rows: vec![
                vec![json!("A-1"), json!(12), json!("OUTSIDE-COL-1")],
                vec![json!("A-2"), json!(18), json!("OUTSIDE-COL-2")],
                vec![json!("OUTSIDE-ROW"), json!(99), Value::Null],
            ],
            persisted_rows: 12,
            truncated: true,
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

    #[test]
    fn authorized_projection_renders_five_real_tabs_without_copying_outside_the_viewport() {
        let artifact = artifact();
        let preview = preview();
        let projection = ArtifactWorkspaceProjection::authorized(
            &artifact,
            "Warehouse",
            ResultsAccess::Available(preview.as_view()),
            ResultsViewportRequest {
                row_offset: 0,
                column_offset: 0,
                row_count: 2,
                column_count: 2,
            },
        );
        let mut state = ArtifactWorkspaceState::default();
        assert_eq!(
            state.reduce(ArtifactAction::ProjectionLoaded(projection.clone())),
            ArtifactOutcome::Changed
        );
        let results = render_projected_lines(&state, &projection).join("\n");
        assert!(results.contains("[Results]"));
        assert!(results.contains("A-1"));
        assert!(results.contains("UI preview limited"));
        assert!(!results.contains("OUTSIDE-ROW"));
        assert!(!results.contains("OUTSIDE-COL"));

        state.reduce(ArtifactAction::NextTab);
        let sql = render_projected_lines(&state, &projection).join("\n");
        assert!(sql.contains("[SQL]"));
        assert!(sql.contains("view only · no rerun"));
        assert!(sql.contains("SELECT count(*)"));

        state.reduce(ArtifactAction::PreviousTab);
        state.reduce(ArtifactAction::PreviousTab);
        let summary = render_projected_lines(&state, &projection).join("\n");
        assert!(summary.contains("There are 42 governed orders"));
        assert!(summary.contains("Query warning · query_result_truncated"));
        assert!(summary.contains("Source · Warehouse"));
        assert!(!summary.contains("internal-source-id"));
        assert!(!summary.contains("raw assumption"));
        assert!(!summary.contains("raw check detail"));

        state.reduce(ArtifactAction::PreviousTab);
        let evidence = render_projected_lines(&state, &projection).join("\n");
        assert!(evidence.contains("[Evidence]"));
        assert!(evidence.contains("policy_scope · passed"));
        assert!(!evidence.contains("raw check detail"));
        state.reduce(ArtifactAction::PreviousTab);
        let schema = render_projected_lines(&state, &projection).join("\n");
        assert!(schema.contains("[Schema]"));
        assert!(schema.contains("Allowed source · analytics.orders"));
    }

    #[test]
    fn missing_and_policy_restricted_artifacts_render_only_the_stable_reason() {
        for (projection, expected) in [
            (
                ArtifactWorkspaceProjection::unavailable(ArtifactUnavailableReason::Missing),
                "Artifact is missing",
            ),
            (
                ArtifactWorkspaceProjection::unavailable(
                    ArtifactUnavailableReason::PolicyRestricted,
                ),
                "Artifact is restricted by Policy",
            ),
        ] {
            let state = ArtifactWorkspaceState::default();
            let rendered = render_projected_lines(&state, &projection).join("\n");
            assert!(rendered.contains(expected));
            assert!(!rendered.contains("SELECT"));
            assert!(!rendered.contains("A-1"));
        }
    }

    #[test]
    fn result_level_missing_and_restricted_states_do_not_hide_other_authorized_tabs() {
        let artifact = artifact();
        for (access, expected) in [
            (ResultsAccess::Missing, "Result Artifact is missing"),
            (
                ResultsAccess::PolicyRestricted,
                "Results are restricted by Policy",
            ),
        ] {
            let projection = ArtifactWorkspaceProjection::authorized(
                &artifact,
                "Warehouse",
                access,
                ResultsViewportRequest {
                    row_offset: 0,
                    column_offset: 0,
                    row_count: 2,
                    column_count: 2,
                },
            );
            let mut state = ArtifactWorkspaceState::default();
            state.reduce(ArtifactAction::ProjectionLoaded(projection.clone()));
            assert!(
                render_projected_lines(&state, &projection)
                    .join("\n")
                    .contains(expected)
            );
            state.reduce(ArtifactAction::NextTab);
            assert!(
                render_projected_lines(&state, &projection)
                    .join("\n")
                    .contains("view only · no rerun")
            );
        }
    }

    #[test]
    fn external_cells_are_control_cleaned_and_width_limited() {
        let artifact = artifact();
        let long_cell = format!("{}\nTOKEN", "x".repeat(400));
        let preview = OwnedAuthorizedResultsPreview {
            columns: vec!["value\tname".to_owned()],
            rows: vec![vec![json!(long_cell)]],
            persisted_rows: 1,
            truncated: false,
        };
        let projection = ArtifactWorkspaceProjection::authorized(
            &artifact,
            "Warehouse",
            ResultsAccess::Available(preview.as_view()),
            ResultsViewportRequest {
                row_offset: 0,
                column_offset: 0,
                row_count: 1,
                column_count: 1,
            },
        );
        let mut state = ArtifactWorkspaceState::default();
        state.reduce(ArtifactAction::ProjectionLoaded(projection.clone()));
        let rendered = render_projected_lines(&state, &projection).join("\n");

        assert!(!rendered.contains('\t'));
        assert!(!rendered.contains("TOKEN"));
        assert!(state.results().rows()[0][0].chars().count() <= MAX_CELL_CHARS);
    }
}
