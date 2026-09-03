mod app;
mod composer;
mod event_loop;
mod input;
mod mode_picker;
mod palette;
pub mod provider_management;
mod theme;
mod ui;

pub use app::{
    AnswerView, BaseView, DetailKind, DetailView, DiagnosticsView, TaskSummary, TranscriptItem,
    TransientView, TuiApp, TuiController,
};
pub use event_loop::{
    AsyncOperationRegistry, ProviderOperationCancellation, ProviderOperationCompletion,
    ProviderOperationPolicy, run_tui,
};
pub use input::{DetailRequest, InputAction, InputError, parse_input};
pub use mode_picker::{ModePickerAction, ModePickerOutcome, ModePickerState, TuiQueryMode};
pub use palette::{SelectionItem, Selector};
pub use theme::{ColorSpec, ThemeRegistry, ThemeToken, UiPreferenceStore, UiPreferences};
pub use ui::{LayoutMode, bottom_panel_height, render, render_to_string};
