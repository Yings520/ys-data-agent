use ys_agent_core::{
    ListModelCandidatesRequest, ModelCandidateBatch, ModelCandidateKey, ModelCandidateStatus,
    ModelCandidateView, ModelSelectionSnapshot, SelectionAvailability, SelectionCurrentStatus,
    SelectionTarget, SelectionTargetView,
};

use super::navigation::FocusTarget;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSelectionState {
    pub search: String,
    pub highlighted: Option<usize>,
    pub scroll: usize,
    pub focus: FocusTarget,
    tab: ModelSelectionTab,
    level: ModelSelectionLevel,
    load_state: ModelSelectionLoadState,
    snapshot: Option<ModelSelectionSnapshot>,
    candidates: Option<ModelCandidateBatch>,
    model_target: Option<SelectionTarget>,
    parent_cursor: Option<ParentCursor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModelSelectionTab {
    #[default]
    Providers,
    Plans,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModelSelectionLevel {
    #[default]
    Targets,
    Models,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ModelSelectionLoadState {
    #[default]
    Loading,
    Ready,
    Empty,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParentCursor {
    tab: ModelSelectionTab,
    search: String,
    highlighted: Option<usize>,
    scroll: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelSelectionAction {
    SnapshotLoaded(ModelSelectionSnapshot),
    SnapshotFailed(String),
    CandidatesLoaded(ModelCandidateBatch),
    CandidatesFailed(String),
    Retry,
    NextTab,
    PreviousTab,
    MoveUp,
    MoveDown,
    SearchChanged(String),
    Confirm,
    Back,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelSelectionBlock {
    TargetUnavailable,
    CapabilityInsufficient,
    ModelUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelSelectionOutcome {
    Changed,
    Ignored,
    ReloadSnapshot,
    LoadCandidates(ListModelCandidatesRequest),
    OpenProviderManagement(SelectionTarget),
    Activate(ModelCandidateKey),
    ValidateThenActivate(ModelCandidateKey),
    RevalidateThenActivate(ModelCandidateKey),
    AlreadyCurrent,
    Blocked(ModelSelectionBlock),
    Close,
}

impl Default for ModelSelectionState {
    fn default() -> Self {
        Self {
            search: String::new(),
            highlighted: None,
            scroll: 0,
            focus: FocusTarget::ModelSelectionList,
            tab: ModelSelectionTab::Providers,
            level: ModelSelectionLevel::Targets,
            load_state: ModelSelectionLoadState::Loading,
            snapshot: None,
            candidates: None,
            model_target: None,
            parent_cursor: None,
        }
    }
}

impl ModelSelectionState {
    pub const fn tab(&self) -> ModelSelectionTab {
        self.tab
    }

    pub const fn level(&self) -> ModelSelectionLevel {
        self.level
    }

    pub const fn load_state(&self) -> &ModelSelectionLoadState {
        &self.load_state
    }

    pub fn visible_targets(&self) -> Vec<&SelectionTargetView> {
        let query = normalized_query(&self.search);
        self.snapshot
            .as_ref()
            .map(ModelSelectionSnapshot::targets)
            .unwrap_or_default()
            .iter()
            .filter(|target| target_in_tab(target.target(), self.tab))
            .filter(|target| matches_query(target.display_name(), &query))
            .collect()
    }

    pub fn visible_candidates(&self) -> Vec<&ModelCandidateView> {
        let query = normalized_query(&self.search);
        self.candidates
            .as_ref()
            .map(ModelCandidateBatch::candidates)
            .unwrap_or_default()
            .iter()
            .filter(|candidate| {
                matches_query(candidate.model_display_name(), &query)
                    || matches_query(candidate.profile_display_name(), &query)
            })
            .collect()
    }

    pub fn visible_target_count(&self) -> usize {
        self.visible_targets().len()
    }

    pub fn visible_candidate_count(&self) -> usize {
        self.visible_candidates().len()
    }

    pub fn current_target_count(&self) -> usize {
        self.snapshot
            .as_ref()
            .map(ModelSelectionSnapshot::targets)
            .unwrap_or_default()
            .iter()
            .filter(|target| target.current() == SelectionCurrentStatus::Current)
            .count()
    }

    pub fn current_candidate_count(&self) -> usize {
        self.candidates
            .as_ref()
            .map(ModelCandidateBatch::candidates)
            .unwrap_or_default()
            .iter()
            .filter(|candidate| candidate.current() == SelectionCurrentStatus::Current)
            .count()
    }

    pub fn has_current_model(&self) -> bool {
        self.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot
                .targets()
                .iter()
                .any(|target| target.current().is_current())
        })
    }

    pub fn reduce(&mut self, action: ModelSelectionAction) -> ModelSelectionOutcome {
        match action {
            ModelSelectionAction::SnapshotLoaded(snapshot) => {
                self.level = ModelSelectionLevel::Targets;
                self.load_state = if snapshot.targets().is_empty() {
                    ModelSelectionLoadState::Empty
                } else {
                    ModelSelectionLoadState::Ready
                };
                self.snapshot = Some(snapshot);
                self.candidates = None;
                self.model_target = None;
                self.parent_cursor = None;
                self.normalize_highlight();
                ModelSelectionOutcome::Changed
            }
            ModelSelectionAction::SnapshotFailed(code) => {
                self.level = ModelSelectionLevel::Targets;
                self.load_state = ModelSelectionLoadState::Failed(code);
                self.snapshot = None;
                self.candidates = None;
                self.model_target = None;
                self.highlighted = None;
                self.scroll = 0;
                ModelSelectionOutcome::Changed
            }
            ModelSelectionAction::CandidatesLoaded(batch) => {
                if self.level != ModelSelectionLevel::Models
                    || self.model_target.as_ref() != Some(batch.target())
                {
                    return ModelSelectionOutcome::Ignored;
                }
                self.load_state = if batch.candidates().is_empty() {
                    ModelSelectionLoadState::Empty
                } else {
                    ModelSelectionLoadState::Ready
                };
                self.candidates = Some(batch);
                self.normalize_highlight();
                ModelSelectionOutcome::Changed
            }
            ModelSelectionAction::CandidatesFailed(code) => {
                if self.level != ModelSelectionLevel::Models {
                    return ModelSelectionOutcome::Ignored;
                }
                self.load_state = ModelSelectionLoadState::Failed(code);
                self.candidates = None;
                self.highlighted = None;
                self.scroll = 0;
                ModelSelectionOutcome::Changed
            }
            ModelSelectionAction::Retry => self.retry(),
            ModelSelectionAction::NextTab | ModelSelectionAction::PreviousTab => {
                if self.level != ModelSelectionLevel::Targets {
                    return ModelSelectionOutcome::Ignored;
                }
                self.tab = match self.tab {
                    ModelSelectionTab::Providers => ModelSelectionTab::Plans,
                    ModelSelectionTab::Plans => ModelSelectionTab::Providers,
                };
                self.search.clear();
                self.scroll = 0;
                self.normalize_highlight();
                ModelSelectionOutcome::Changed
            }
            ModelSelectionAction::MoveUp => {
                self.highlighted = self.highlighted.map(|index| index.saturating_sub(1));
                self.keep_highlight_visible();
                ModelSelectionOutcome::Changed
            }
            ModelSelectionAction::MoveDown => {
                let count = self.visible_count();
                self.highlighted = (count > 0).then(|| {
                    self.highlighted
                        .unwrap_or_default()
                        .saturating_add(1)
                        .min(count - 1)
                });
                self.keep_highlight_visible();
                ModelSelectionOutcome::Changed
            }
            ModelSelectionAction::SearchChanged(query) => {
                self.search = query;
                self.scroll = 0;
                self.normalize_highlight();
                ModelSelectionOutcome::Changed
            }
            ModelSelectionAction::Confirm => self.confirm(),
            ModelSelectionAction::Back => self.back(),
        }
    }

    fn retry(&mut self) -> ModelSelectionOutcome {
        self.load_state = ModelSelectionLoadState::Loading;
        match (&self.level, self.model_target.clone()) {
            (ModelSelectionLevel::Models, Some(target)) => {
                ModelSelectionOutcome::LoadCandidates(ListModelCandidatesRequest { target })
            }
            _ => ModelSelectionOutcome::ReloadSnapshot,
        }
    }

    fn confirm(&mut self) -> ModelSelectionOutcome {
        if self.load_state != ModelSelectionLoadState::Ready {
            return ModelSelectionOutcome::Ignored;
        }
        match self.level {
            ModelSelectionLevel::Targets => self.confirm_target(),
            ModelSelectionLevel::Models => self.confirm_candidate(),
        }
    }

    fn confirm_target(&mut self) -> ModelSelectionOutcome {
        let Some(target) = self.selected_target().cloned() else {
            return ModelSelectionOutcome::Ignored;
        };
        match target.availability() {
            SelectionAvailability::NeedsSetup => {
                ModelSelectionOutcome::OpenProviderManagement(target.target().clone())
            }
            SelectionAvailability::Unavailable => {
                ModelSelectionOutcome::Blocked(ModelSelectionBlock::TargetUnavailable)
            }
            SelectionAvailability::Configured => {
                let selected_target = target.target().clone();
                self.parent_cursor = Some(ParentCursor {
                    tab: self.tab,
                    search: self.search.clone(),
                    highlighted: self.highlighted,
                    scroll: self.scroll,
                });
                self.level = ModelSelectionLevel::Models;
                self.load_state = ModelSelectionLoadState::Loading;
                self.model_target = Some(selected_target.clone());
                self.candidates = None;
                self.search.clear();
                self.highlighted = None;
                self.scroll = 0;
                ModelSelectionOutcome::LoadCandidates(ListModelCandidatesRequest {
                    target: selected_target,
                })
            }
        }
    }

    fn confirm_candidate(&self) -> ModelSelectionOutcome {
        let Some(candidate) = self.selected_candidate() else {
            return ModelSelectionOutcome::Ignored;
        };
        if candidate.current().is_current() {
            return ModelSelectionOutcome::AlreadyCurrent;
        }
        match candidate.status() {
            ModelCandidateStatus::Ready => ModelSelectionOutcome::Activate(candidate.key().clone()),
            ModelCandidateStatus::NeedsValidation => {
                ModelSelectionOutcome::ValidateThenActivate(candidate.key().clone())
            }
            ModelCandidateStatus::ValidationExpired => {
                ModelSelectionOutcome::RevalidateThenActivate(candidate.key().clone())
            }
            ModelCandidateStatus::CapabilityInsufficient => {
                ModelSelectionOutcome::Blocked(ModelSelectionBlock::CapabilityInsufficient)
            }
            ModelCandidateStatus::Unavailable => {
                ModelSelectionOutcome::Blocked(ModelSelectionBlock::ModelUnavailable)
            }
        }
    }

    fn back(&mut self) -> ModelSelectionOutcome {
        if self.level == ModelSelectionLevel::Targets {
            return ModelSelectionOutcome::Close;
        }
        let Some(parent) = self.parent_cursor.take() else {
            return ModelSelectionOutcome::Ignored;
        };
        self.level = ModelSelectionLevel::Targets;
        self.load_state = if self
            .snapshot
            .as_ref()
            .is_none_or(|snapshot| snapshot.targets().is_empty())
        {
            ModelSelectionLoadState::Empty
        } else {
            ModelSelectionLoadState::Ready
        };
        self.tab = parent.tab;
        self.search = parent.search;
        self.highlighted = parent.highlighted;
        self.scroll = parent.scroll;
        self.candidates = None;
        self.model_target = None;
        self.normalize_highlight();
        ModelSelectionOutcome::Changed
    }

    fn selected_target(&self) -> Option<&SelectionTargetView> {
        self.highlighted
            .and_then(|selected| self.visible_targets().get(selected).copied())
    }

    fn selected_candidate(&self) -> Option<&ModelCandidateView> {
        self.highlighted
            .and_then(|selected| self.visible_candidates().get(selected).copied())
    }

    fn visible_count(&self) -> usize {
        match self.level {
            ModelSelectionLevel::Targets => self.visible_target_count(),
            ModelSelectionLevel::Models => self.visible_candidate_count(),
        }
    }

    fn normalize_highlight(&mut self) {
        let count = self.visible_count();
        self.highlighted = (count > 0).then(|| self.highlighted.unwrap_or_default().min(count - 1));
        self.keep_highlight_visible();
    }

    fn keep_highlight_visible(&mut self) {
        const VISIBLE_ROWS: usize = 6;
        let Some(selected) = self.highlighted else {
            self.scroll = 0;
            return;
        };
        if selected < self.scroll {
            self.scroll = selected;
        } else if selected >= self.scroll.saturating_add(VISIBLE_ROWS) {
            self.scroll = selected + 1 - VISIBLE_ROWS;
        }
    }
}

fn target_in_tab(target: &SelectionTarget, tab: ModelSelectionTab) -> bool {
    matches!(
        (target, tab),
        (SelectionTarget::Provider(_), ModelSelectionTab::Providers)
            | (SelectionTarget::Plan { .. }, ModelSelectionTab::Plans)
    )
}

fn normalized_query(query: &str) -> String {
    query.trim().to_ascii_lowercase()
}

fn matches_query(value: &str, query: &str) -> bool {
    query.is_empty() || value.to_ascii_lowercase().contains(query)
}

pub fn render_lines(state: &ModelSelectionState) -> Vec<String> {
    let mut lines = vec!["Model Selection".to_owned()];
    if !state.search.is_empty() {
        lines.push(format!("Search · {}", state.search));
    }
    lines
}

#[cfg(test)]
mod tests {
    use ys_agent_core::{
        ModelCandidateBatch, ModelCandidateKey, ModelCandidateStatus, ModelCandidateView,
        ModelSelectionSnapshot, ProfileId, ProviderId, ProviderModelId, ProviderPlanId,
        SelectionAvailability, SelectionCurrentStatus, SelectionTarget, SelectionTargetView,
    };

    use super::*;

    fn target(
        target: SelectionTarget,
        label: &str,
        availability: SelectionAvailability,
        current: SelectionCurrentStatus,
    ) -> SelectionTargetView {
        SelectionTargetView::new(target, label, availability, current).expect("valid target")
    }

    fn snapshot() -> ModelSelectionSnapshot {
        ModelSelectionSnapshot::new(vec![
            target(
                SelectionTarget::Provider(ProviderId::DeepSeek),
                "DeepSeek",
                SelectionAvailability::Configured,
                SelectionCurrentStatus::Current,
            ),
            target(
                SelectionTarget::Provider(ProviderId::Xai),
                "xAI",
                SelectionAvailability::NeedsSetup,
                SelectionCurrentStatus::NotCurrent,
            ),
            target(
                SelectionTarget::Provider(ProviderId::Anthropic),
                "Anthropic",
                SelectionAvailability::Unavailable,
                SelectionCurrentStatus::NotCurrent,
            ),
            target(
                SelectionTarget::Plan {
                    provider: ProviderId::ChatGptSubscription,
                    plan: ProviderPlanId::new("plus").expect("valid plan"),
                },
                "ChatGPT Plus",
                SelectionAvailability::Configured,
                SelectionCurrentStatus::NotCurrent,
            ),
        ])
        .expect("single current target")
    }

    fn candidate(status: ModelCandidateStatus, suffix: &str) -> ModelCandidateView {
        let provider = ProviderId::DeepSeek;
        let model =
            ProviderModelId::new(provider, format!("deepseek/{suffix}")).expect("valid model id");
        ModelCandidateView::new(
            ModelCandidateKey::new(ProfileId::new(), 1, Some(1), provider, model)
                .expect("valid candidate key"),
            format!("Profile {suffix}"),
            format!("Model {suffix}"),
            status,
            SelectionCurrentStatus::NotCurrent,
        )
        .expect("valid candidate")
    }

    fn current_candidate() -> ModelCandidateView {
        let provider = ProviderId::DeepSeek;
        let model = ProviderModelId::new(provider, "deepseek/current").expect("valid model id");
        ModelCandidateView::new(
            ModelCandidateKey::new(ProfileId::new(), 1, Some(1), provider, model)
                .expect("valid candidate key"),
            "Current Profile",
            "Current Model",
            ModelCandidateStatus::Ready,
            SelectionCurrentStatus::Current,
        )
        .expect("valid current candidate")
    }

    fn enter_models(state: &mut ModelSelectionState) {
        assert!(matches!(
            state.reduce(ModelSelectionAction::Confirm),
            ModelSelectionOutcome::LoadCandidates(_)
        ));
        let batch = ModelCandidateBatch::new(
            SelectionTarget::Provider(ProviderId::DeepSeek),
            vec![
                candidate(ModelCandidateStatus::Ready, "ready"),
                candidate(ModelCandidateStatus::NeedsValidation, "new"),
                candidate(ModelCandidateStatus::ValidationExpired, "expired"),
                candidate(ModelCandidateStatus::CapabilityInsufficient, "insufficient"),
                candidate(ModelCandidateStatus::Unavailable, "unavailable"),
            ],
        )
        .expect("matching batch");
        assert_eq!(
            state.reduce(ModelSelectionAction::CandidatesLoaded(batch)),
            ModelSelectionOutcome::Changed
        );
    }

    #[test]
    fn tabs_search_and_child_back_restore_the_exact_parent_cursor() {
        let mut state = ModelSelectionState::default();
        assert_eq!(
            state.reduce(ModelSelectionAction::SnapshotLoaded(snapshot())),
            ModelSelectionOutcome::Changed
        );
        assert_eq!(state.tab(), ModelSelectionTab::Providers);
        assert_eq!(state.visible_target_count(), 3);
        assert_eq!(state.current_target_count(), 1);

        state.reduce(ModelSelectionAction::SearchChanged("deep".to_owned()));
        let parent_highlight = state.highlighted;
        enter_models(&mut state);
        assert_eq!(state.level(), ModelSelectionLevel::Models);
        assert_eq!(
            state.reduce(ModelSelectionAction::Back),
            ModelSelectionOutcome::Changed
        );
        assert_eq!(state.level(), ModelSelectionLevel::Targets);
        assert_eq!(state.tab(), ModelSelectionTab::Providers);
        assert_eq!(state.search, "deep");
        assert_eq!(state.highlighted, parent_highlight);

        state.reduce(ModelSelectionAction::NextTab);
        assert_eq!(state.tab(), ModelSelectionTab::Plans);
        assert_eq!(state.visible_target_count(), 1);
        state.reduce(ModelSelectionAction::PreviousTab);
        assert_eq!(state.tab(), ModelSelectionTab::Providers);
    }

    #[test]
    fn target_and_model_statuses_produce_distinct_typed_intents() {
        let mut state = ModelSelectionState::default();
        state.reduce(ModelSelectionAction::SnapshotLoaded(snapshot()));

        state.reduce(ModelSelectionAction::SearchChanged("xai".to_owned()));
        assert!(matches!(
            state.reduce(ModelSelectionAction::Confirm),
            ModelSelectionOutcome::OpenProviderManagement(_)
        ));
        state.reduce(ModelSelectionAction::SearchChanged("anthropic".to_owned()));
        assert_eq!(
            state.reduce(ModelSelectionAction::Confirm),
            ModelSelectionOutcome::Blocked(ModelSelectionBlock::TargetUnavailable)
        );

        state.reduce(ModelSelectionAction::SearchChanged("deep".to_owned()));
        enter_models(&mut state);
        assert!(matches!(
            state.reduce(ModelSelectionAction::Confirm),
            ModelSelectionOutcome::Activate(_)
        ));
        state.reduce(ModelSelectionAction::MoveDown);
        assert!(matches!(
            state.reduce(ModelSelectionAction::Confirm),
            ModelSelectionOutcome::ValidateThenActivate(_)
        ));
        state.reduce(ModelSelectionAction::MoveDown);
        assert!(matches!(
            state.reduce(ModelSelectionAction::Confirm),
            ModelSelectionOutcome::RevalidateThenActivate(_)
        ));
        state.reduce(ModelSelectionAction::MoveDown);
        assert_eq!(
            state.reduce(ModelSelectionAction::Confirm),
            ModelSelectionOutcome::Blocked(ModelSelectionBlock::CapabilityInsufficient)
        );
        state.reduce(ModelSelectionAction::MoveDown);
        assert_eq!(
            state.reduce(ModelSelectionAction::Confirm),
            ModelSelectionOutcome::Blocked(ModelSelectionBlock::ModelUnavailable)
        );
    }

    #[test]
    fn empty_and_failed_catalogs_remain_retryable_without_fake_rows() {
        let mut state = ModelSelectionState::default();
        state.reduce(ModelSelectionAction::SnapshotLoaded(
            ModelSelectionSnapshot::new(Vec::new()).expect("empty snapshot"),
        ));
        assert_eq!(state.load_state(), &ModelSelectionLoadState::Empty);
        assert_eq!(state.visible_target_count(), 0);
        assert_eq!(
            state.reduce(ModelSelectionAction::Retry),
            ModelSelectionOutcome::ReloadSnapshot
        );

        state.reduce(ModelSelectionAction::SnapshotFailed(
            "provider.catalog_unavailable".to_owned(),
        ));
        assert_eq!(
            state.load_state(),
            &ModelSelectionLoadState::Failed("provider.catalog_unavailable".to_owned())
        );
        assert_eq!(state.visible_target_count(), 0);
        assert_eq!(
            state.reduce(ModelSelectionAction::Retry),
            ModelSelectionOutcome::ReloadSnapshot
        );
    }

    #[test]
    fn a_search_with_no_matches_does_not_turn_a_loaded_catalog_into_an_empty_catalog() {
        let mut state = ModelSelectionState::default();
        state.reduce(ModelSelectionAction::SnapshotLoaded(snapshot()));
        state.reduce(ModelSelectionAction::SearchChanged(
            "definitely-not-a-provider".to_owned(),
        ));

        assert_eq!(state.visible_target_count(), 0);
        assert_eq!(state.load_state(), &ModelSelectionLoadState::Ready);
    }

    #[test]
    fn loading_state_cannot_emit_an_intent_from_stale_rows() {
        let mut state = ModelSelectionState::default();
        state.reduce(ModelSelectionAction::SnapshotLoaded(snapshot()));
        assert_eq!(
            state.reduce(ModelSelectionAction::Retry),
            ModelSelectionOutcome::ReloadSnapshot
        );
        assert_eq!(
            state.reduce(ModelSelectionAction::Confirm),
            ModelSelectionOutcome::Ignored
        );
    }

    #[test]
    fn the_current_model_is_unique_and_confirming_it_is_a_no_op() {
        let mut state = ModelSelectionState::default();
        state.reduce(ModelSelectionAction::SnapshotLoaded(snapshot()));
        assert!(matches!(
            state.reduce(ModelSelectionAction::Confirm),
            ModelSelectionOutcome::LoadCandidates(_)
        ));
        state.reduce(ModelSelectionAction::CandidatesLoaded(
            ModelCandidateBatch::new(
                SelectionTarget::Provider(ProviderId::DeepSeek),
                vec![current_candidate()],
            )
            .expect("single current model"),
        ));

        assert_eq!(state.current_candidate_count(), 1);
        assert_eq!(
            state.reduce(ModelSelectionAction::Confirm),
            ModelSelectionOutcome::AlreadyCurrent
        );
    }

    #[test]
    fn returning_from_models_restores_a_scrolled_parent_selection() {
        let targets = ProviderId::ALL
            .into_iter()
            .enumerate()
            .map(|(index, provider)| {
                target(
                    SelectionTarget::Provider(provider),
                    &format!("Provider {index}"),
                    SelectionAvailability::Configured,
                    if index == 0 {
                        SelectionCurrentStatus::Current
                    } else {
                        SelectionCurrentStatus::NotCurrent
                    },
                )
            })
            .collect();
        let mut state = ModelSelectionState::default();
        state.reduce(ModelSelectionAction::SnapshotLoaded(
            ModelSelectionSnapshot::new(targets).expect("unique current"),
        ));
        for _ in 0..8 {
            state.reduce(ModelSelectionAction::MoveDown);
        }
        let parent_highlight = state.highlighted;
        let parent_scroll = state.scroll;
        let request = match state.reduce(ModelSelectionAction::Confirm) {
            ModelSelectionOutcome::LoadCandidates(request) => request,
            outcome => panic!("expected model load, got {outcome:?}"),
        };
        state.reduce(ModelSelectionAction::CandidatesLoaded(
            ModelCandidateBatch::new(request.target, Vec::new()).expect("matching empty batch"),
        ));
        state.reduce(ModelSelectionAction::Back);

        assert_eq!(state.highlighted, parent_highlight);
        assert_eq!(state.scroll, parent_scroll);
    }
}
