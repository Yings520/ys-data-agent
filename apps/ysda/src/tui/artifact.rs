use super::navigation::FocusTarget;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactWorkspaceState {
    pub search: String,
    pub highlighted: Option<usize>,
    pub scroll: usize,
    pub focus: FocusTarget,
}

impl Default for ArtifactWorkspaceState {
    fn default() -> Self {
        Self {
            search: String::new(),
            highlighted: None,
            scroll: 0,
            focus: FocusTarget::ArtifactContent,
        }
    }
}

pub fn render_lines(state: &ArtifactWorkspaceState) -> Vec<String> {
    let mut lines = vec!["Artifact".to_owned()];
    if !state.search.is_empty() {
        lines.push(format!("Search · {}", state.search));
    }
    lines
}
