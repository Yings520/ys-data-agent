mod app;
mod artifact;
mod async_guard;
mod composer;
mod event_loop;
mod input;
mod mode_picker;
mod model_selection;
mod navigation;
mod palette;
pub mod provider_management;
mod theme;
mod timeline;
mod ui;

pub use app::{
    AnswerView, BaseView, DetailKind, DetailView, DiagnosticsView, DisplayContextRefreshTrigger,
    TaskSummary, TranscriptItem, TransientView, TuiApp, TuiController,
};
pub use artifact::ArtifactWorkspaceState;
pub use async_guard::{AsyncChannel, AsyncOperationBusy, AsyncOperationTicket, AsyncResultGuard};
pub use event_loop::{
    AsyncOperationRegistry, ProviderOperationCancellation, ProviderOperationCompletion,
    ProviderOperationPolicy, run_tui,
};
pub use input::{DetailRequest, InputAction, InputError, parse_input};
pub use mode_picker::{ModePickerAction, ModePickerOutcome, ModePickerState, TuiQueryMode};
pub use model_selection::{
    ModelSelectionAction, ModelSelectionBlock, ModelSelectionLevel, ModelSelectionLoadState,
    ModelSelectionOutcome, ModelSelectionState, ModelSelectionTab,
};
pub use navigation::{
    ContentRoute, FocusTarget, InputLayer, NavigationState, Overlay, ProviderNavigationState,
    RouteKey,
};
pub use palette::{SelectionItem, Selector};
pub use theme::{ColorSpec, ThemeRegistry, ThemeToken, UiPreferenceStore, UiPreferences};
pub use timeline::{HitRegion, TimelineState};
pub use ui::{LayoutMode, bottom_panel_height, render, render_to_string};
