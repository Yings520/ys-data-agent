use super::navigation::FocusTarget;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSelectionState {
    pub search: String,
    pub highlighted: Option<usize>,
    pub scroll: usize,
    pub focus: FocusTarget,
}

impl Default for ModelSelectionState {
    fn default() -> Self {
        Self {
            search: String::new(),
            highlighted: None,
            scroll: 0,
            focus: FocusTarget::ModelSelectionList,
        }
    }
}

pub fn render_lines(state: &ModelSelectionState) -> Vec<String> {
    let mut lines = vec!["Model Selection".to_owned()];
    if !state.search.is_empty() {
        lines.push(format!("Search · {}", state.search));
    }
    lines
}
