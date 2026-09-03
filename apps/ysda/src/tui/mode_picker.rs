use ys_agent_core::WorkflowKind;

use super::palette::{SelectionItem, Selector};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiQueryMode {
    Auto,
    Query,
}

impl TuiQueryMode {
    pub const fn workflow(self) -> WorkflowKind {
        match self {
            Self::Auto | Self::Query => WorkflowKind::Query,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Query => "Query",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModeOption {
    mode: TuiQueryMode,
    description: &'static str,
}

impl SelectionItem for ModeOption {
    fn search_name(&self) -> &str {
        self.mode.label()
    }

    fn search_description(&self) -> &str {
        self.description
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModePickerAction {
    MoveUp,
    MoveDown,
    Insert(char),
    Backspace,
    Confirm,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModePickerOutcome {
    Open,
    Confirmed(TuiQueryMode),
    Cancelled {
        mode: TuiQueryMode,
        composer: String,
    },
}

#[derive(Debug, Clone)]
pub struct ModePickerState {
    selector: Selector<ModeOption>,
    query: String,
    original_mode: TuiQueryMode,
    original_composer: String,
}

impl ModePickerState {
    pub fn new(original_mode: TuiQueryMode, original_composer: String) -> Self {
        let mut selector = Selector::new(mode_options());
        if original_mode == TuiQueryMode::Query {
            selector.move_down();
        }
        Self {
            selector,
            query: String::new(),
            original_mode,
            original_composer,
        }
    }

    pub const fn options(&self) -> [TuiQueryMode; 2] {
        [TuiQueryMode::Auto, TuiQueryMode::Query]
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn rows(&self) -> impl Iterator<Item = (bool, TuiQueryMode)> + '_ {
        self.selector
            .rows()
            .map(|(selected, option)| (selected, option.mode))
    }

    pub fn reduce(&mut self, action: ModePickerAction) -> ModePickerOutcome {
        match action {
            ModePickerAction::MoveUp => self.selector.move_up(),
            ModePickerAction::MoveDown => self.selector.move_down(),
            ModePickerAction::Insert(character) if !character.is_control() => {
                self.query.push(character);
                self.selector.update_query(&self.query);
            }
            ModePickerAction::Backspace => {
                self.query.pop();
                self.selector.update_query(&self.query);
            }
            ModePickerAction::Confirm => {
                if let Some(option) = self.selector.selected() {
                    return ModePickerOutcome::Confirmed(option.mode);
                }
            }
            ModePickerAction::Cancel => {
                return ModePickerOutcome::Cancelled {
                    mode: self.original_mode,
                    composer: self.original_composer.clone(),
                };
            }
            ModePickerAction::Insert(_) => {}
        }
        ModePickerOutcome::Open
    }
}

fn mode_options() -> Vec<ModeOption> {
    vec![
        ModeOption {
            mode: TuiQueryMode::Auto,
            description: "automatically resolve to Query in v0.2",
        },
        ModeOption {
            mode: TuiQueryMode::Query,
            description: "explicitly use the governed Query workflow",
        },
    ]
}
