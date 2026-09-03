//! Reserved for the Provider-management TUI reducer and view model (task 6.1).
//!
//! The screen remains I/O-free and reaches runtime behavior only through the
//! existing service API when its implementation is added.

use std::collections::BTreeMap;

use ys_agent_core::{
    ActiveProviderView, CompatibilityEvidenceView, CredentialKind, CredentialViewStatus,
    DeviceAuthorizationView, DiscoveredModel, OAuthConnectionStatus, OperationId,
    ParameterApplicability, ProfileDetail, ProfileId, ProfileState, ProviderCatalogView,
    ProviderField, ProviderId, ProviderManagementError, ProviderModelId, ProviderParameterKey,
    ProviderParameters, ProviderRemediation, SecretValue,
};
use zeroize::{Zeroize, Zeroizing};

const SECRET_MASK: &str = "••••••••";
const VISIBLE_MODEL_ROWS: usize = 9;

/// The fixed, ordered Provider-management wizard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderManagementStep {
    Provider,
    Authentication,
    Model,
    Parameters,
    Validate,
    SaveActivate,
}

impl ProviderManagementStep {
    fn next(self) -> Option<Self> {
        match self {
            Self::Provider => Some(Self::Authentication),
            Self::Authentication => Some(Self::Model),
            Self::Model => Some(Self::Parameters),
            Self::Parameters => Some(Self::Validate),
            Self::Validate => Some(Self::SaveActivate),
            Self::SaveActivate => None,
        }
    }

    fn previous(self) -> Option<Self> {
        match self {
            Self::Provider => None,
            Self::Authentication => Some(Self::Provider),
            Self::Model => Some(Self::Authentication),
            Self::Parameters => Some(Self::Model),
            Self::Validate => Some(Self::Parameters),
            Self::SaveActivate => Some(Self::Validate),
        }
    }
}

/// A non-sensitive summary used to render one locally known Profile.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderProfileView {
    pub profile_id: ProfileId,
    pub name: String,
    pub provider: ProviderId,
    pub revision: u64,
    pub model: ProviderModelId,
    pub parameters: ProviderParameters,
    pub state: ProfileState,
    pub credential_status: CredentialViewStatus,
    pub oauth_status: Option<OAuthConnectionStatus>,
    pub validation: Option<CompatibilityEvidenceView>,
    pub is_active: bool,
}

impl ProviderProfileView {
    /// Creates a render-safe view from the core's already-masked detail view.
    pub fn from_detail(
        detail: ProfileDetail,
        validation: Option<CompatibilityEvidenceView>,
    ) -> Self {
        Self {
            profile_id: detail.summary.profile_id,
            name: detail.summary.name,
            provider: detail.summary.provider,
            revision: detail.revision,
            model: detail.model,
            parameters: detail.parameters,
            state: detail.summary.state,
            credential_status: detail.summary.credential_status,
            oauth_status: detail.oauth_status,
            validation,
            // The active badge is derived from the committed snapshot below, never copied from a
            // current Profile summary whose revision could already have become Draft or Invalid.
            is_active: false,
        }
    }

    fn is_active_eligible(&self) -> bool {
        self.state == ProfileState::Ready
            && self.credential_status == CredentialViewStatus::Saved
            && self
                .oauth_status
                .as_ref()
                .is_none_or(|status| *status == OAuthConnectionStatus::Connected)
            && self.validation.as_ref().is_none_or(|evidence| {
                evidence.state == ProfileState::Ready
                    && evidence.credential_status == CredentialViewStatus::Saved
                    && evidence.error.is_none()
            })
    }
}

/// Browse data is supplied only from catalog/Profile/active-snapshot reads, so it remains useful
/// offline. It has no Credential value or locator field.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderManagementView {
    pub catalog: Vec<ProviderCatalogView>,
    pub profiles: Vec<ProviderProfileView>,
    pub active: Option<ActiveProviderView>,
    pub offline: bool,
}

impl ProviderManagementView {
    pub fn new(
        catalog: Vec<ProviderCatalogView>,
        mut profiles: Vec<ProviderProfileView>,
        active: Option<ActiveProviderView>,
        offline: bool,
    ) -> Self {
        for profile in &mut profiles {
            profile.is_active = active.as_ref().is_some_and(|snapshot| {
                snapshot.profile_id == profile.profile_id
                    && snapshot.profile_revision == profile.revision
                    && profile.is_active_eligible()
            });
        }

        let active = active.filter(|snapshot| {
            profiles.iter().any(|profile| {
                profile.is_active
                    && profile.profile_id == snapshot.profile_id
                    && profile.revision == snapshot.profile_revision
            })
        });

        Self {
            catalog,
            profiles,
            active,
            offline,
        }
    }

    pub fn offline() -> Self {
        Self::new(Vec::new(), Vec::new(), None, true)
    }
}

/// Authentication selection is intentionally only a kind; it never contains a Credential value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderAuthentication {
    ApiKey,
    OAuth,
}

impl From<CredentialKind> for ProviderAuthentication {
    fn from(value: CredentialKind) -> Self {
        match value {
            CredentialKind::ApiKey => Self::ApiKey,
            CredentialKind::OAuthConnection => Self::OAuth,
        }
    }
}

/// Whether the selected model came from discovery or was entered manually. Both paths are
/// subjected to the same core prefix invariant before entering the edit buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderModelSource {
    Discovered,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderManagementStateKind {
    Browse,
    Edit,
    Confirm,
    Busy,
    Result,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderOperationKind {
    DiscoverModels,
    Validate,
    SaveDraft,
    Activate,
    OAuth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderManagementAction {
    SaveDraft,
    Activate,
}

impl From<ProviderManagementAction> for ProviderOperationKind {
    fn from(value: ProviderManagementAction) -> Self {
        match value {
            ProviderManagementAction::SaveDraft => Self::SaveDraft,
            ProviderManagementAction::Activate => Self::Activate,
        }
    }
}

/// An explicit I/O request emitted by the reducer. The event loop owns all I/O and supplies the
/// matching `OperationId` when it changes the screen to `Busy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderScreenRequest {
    Operation(ProviderOperationKind),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderEditIssue {
    InvalidModel,
    UnsupportedParameters,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderEditView {
    pub profile_id: Option<ProfileId>,
    pub name: String,
    pub provider: Option<ProviderId>,
    pub authentication: Option<ProviderAuthentication>,
    pub model: Option<ProviderModelId>,
    pub model_source: Option<ProviderModelSource>,
    pub discovered_models: Vec<DiscoveredModel>,
    pub model_filter: String,
    pub highlighted_model: Option<usize>,
    pub model_scroll: usize,
    pub parameters: ProviderParameters,
    pub credential_mask: Option<&'static str>,
    pub has_saved_credential: bool,
    pub parameter_issue: Option<ProviderEditIssue>,
}

/// The non-sensitive half of an edit command. It is deliberately separate from `SecretValue` so
/// the event loop can construct revision/validation requests without copying a Credential into a
/// view, command log, or retry buffer.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderEditCommand {
    pub profile_id: Option<ProfileId>,
    pub name: String,
    pub provider: ProviderId,
    pub authentication: ProviderAuthentication,
    pub model: Option<ProviderModelId>,
    pub parameters: ProviderParameters,
    pub observed_context_limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderConfirmationView {
    pub action: ProviderManagementAction,
    pub affects_new_runs_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderBusyView {
    pub operation_id: OperationId,
    pub kind: ProviderOperationKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderResultOutcome {
    Succeeded,
    Failed(ProviderManagementError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderResultView {
    pub operation: ProviderOperationKind,
    pub outcome: ProviderResultOutcome,
    pub remediation: Option<ProviderRemediation>,
    pub can_retry: bool,
}

/// The complete public render model. It deliberately exposes only `ProviderEditView`, whose
/// fixed mask is the sole representation of a typed secret.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderManagementScreenView {
    pub state: ProviderManagementStateKind,
    pub browse: ProviderManagementView,
    pub step: Option<ProviderManagementStep>,
    pub edit: Option<ProviderEditView>,
    pub confirmation: Option<ProviderConfirmationView>,
    pub busy: Option<ProviderBusyView>,
    pub result: Option<ProviderResultView>,
    pub oauth_authorization: Option<DeviceAuthorizationView>,
}

struct ProviderEditBuffer {
    profile_id: Option<ProfileId>,
    name: String,
    provider: Option<ProviderId>,
    authentication: Option<ProviderAuthentication>,
    model: Option<ProviderModelId>,
    model_input: String,
    model_source: Option<ProviderModelSource>,
    discovered_models: Vec<DiscoveredModel>,
    model_filter: String,
    highlighted_model: Option<usize>,
    model_scroll: usize,
    parameters: ProviderParameters,
    credential: Zeroizing<String>,
    has_saved_credential: bool,
    parameter_issue: Option<ProviderEditIssue>,
}

impl ProviderEditBuffer {
    fn new(profile_id: Option<ProfileId>, name: String) -> Self {
        Self {
            profile_id,
            name,
            provider: None,
            authentication: None,
            model: None,
            model_input: String::new(),
            model_source: None,
            discovered_models: Vec::new(),
            model_filter: String::new(),
            highlighted_model: None,
            model_scroll: 0,
            parameters: ProviderParameters::default(),
            credential: Zeroizing::new(String::new()),
            has_saved_credential: false,
            parameter_issue: None,
        }
    }

    fn safe_copy(&self) -> Self {
        Self {
            profile_id: self.profile_id,
            name: self.name.clone(),
            provider: self.provider,
            authentication: self.authentication,
            model: self.model.clone(),
            model_input: self.model_input.clone(),
            model_source: self.model_source,
            discovered_models: self.discovered_models.clone(),
            model_filter: self.model_filter.clone(),
            highlighted_model: self.highlighted_model,
            model_scroll: self.model_scroll,
            parameters: self.parameters.clone(),
            credential: Zeroizing::new(String::new()),
            has_saved_credential: self.has_saved_credential,
            parameter_issue: self.parameter_issue.clone(),
        }
    }

    fn view(&self) -> ProviderEditView {
        ProviderEditView {
            profile_id: self.profile_id,
            name: self.name.clone(),
            provider: self.provider,
            authentication: self.authentication,
            model: self.model.clone(),
            model_source: self.model_source,
            discovered_models: self.discovered_models.clone(),
            model_filter: self.model_filter.clone(),
            highlighted_model: self.highlighted_model,
            model_scroll: self.model_scroll,
            parameters: self.parameters.clone(),
            credential_mask: (!self.credential.is_empty() || self.has_saved_credential)
                .then_some(SECRET_MASK),
            has_saved_credential: self.has_saved_credential && self.credential.is_empty(),
            parameter_issue: self.parameter_issue.clone(),
        }
    }
}

enum ScreenState {
    Browse,
    Edit {
        step: ProviderManagementStep,
        buffer: ProviderEditBuffer,
    },
    Confirm {
        action: ProviderManagementAction,
        buffer: ProviderEditBuffer,
    },
    Busy {
        operation_id: OperationId,
        kind: ProviderOperationKind,
        resume_step: ProviderManagementStep,
        buffer: ProviderEditBuffer,
    },
    Result {
        operation: ProviderOperationKind,
        resume_step: ProviderManagementStep,
        outcome: ProviderResultOutcome,
        buffer: ProviderEditBuffer,
    },
}

/// I/O-free Provider-management reducer. It neither owns a service nor makes SQLite, vault, or
/// network calls; its only sensitive field is a zeroizing input buffer that never crosses the
/// render view.
pub struct ProviderManagementScreen {
    browse: ProviderManagementView,
    state: ScreenState,
    oauth_authorization: Option<DeviceAuthorizationView>,
}

impl ProviderManagementScreen {
    pub fn new(browse: ProviderManagementView) -> Self {
        Self {
            browse,
            state: ScreenState::Browse,
            oauth_authorization: None,
        }
    }

    pub fn state_kind(&self) -> ProviderManagementStateKind {
        match self.state {
            ScreenState::Browse => ProviderManagementStateKind::Browse,
            ScreenState::Edit { .. } => ProviderManagementStateKind::Edit,
            ScreenState::Confirm { .. } => ProviderManagementStateKind::Confirm,
            ScreenState::Busy { .. } => ProviderManagementStateKind::Busy,
            ScreenState::Result { .. } => ProviderManagementStateKind::Result,
        }
    }

    pub fn view(&self) -> ProviderManagementScreenView {
        let mut view = ProviderManagementScreenView {
            state: self.state_kind(),
            browse: self.browse.clone(),
            step: None,
            edit: None,
            confirmation: None,
            busy: None,
            result: None,
            oauth_authorization: self.oauth_authorization.clone(),
        };
        match &self.state {
            ScreenState::Browse => {}
            ScreenState::Edit { step, buffer } => {
                view.step = Some(*step);
                view.edit = Some(buffer.view());
            }
            ScreenState::Confirm { action, buffer } => {
                view.step = Some(ProviderManagementStep::SaveActivate);
                view.edit = Some(buffer.view());
                view.confirmation = Some(ProviderConfirmationView {
                    action: *action,
                    affects_new_runs_only: *action == ProviderManagementAction::Activate,
                });
            }
            ScreenState::Busy {
                operation_id,
                kind,
                buffer,
                ..
            } => {
                view.edit = Some(buffer.view());
                view.busy = Some(ProviderBusyView {
                    operation_id: *operation_id,
                    kind: *kind,
                });
            }
            ScreenState::Result {
                operation,
                resume_step,
                outcome,
                buffer,
            } => {
                view.edit = Some(buffer.view());
                let error = match outcome {
                    ProviderResultOutcome::Succeeded => None,
                    ProviderResultOutcome::Failed(error) => Some(error),
                };
                view.result = Some(ProviderResultView {
                    operation: *operation,
                    outcome: outcome.clone(),
                    remediation: error.map(ProviderManagementError::remediation),
                    can_retry: error.is_some_and(|error| {
                        retry_is_safe(*operation, *resume_step, buffer, error)
                    }),
                });
            }
        }
        view
    }

    pub fn edit_command(&self) -> Option<ProviderEditCommand> {
        let buffer = match &self.state {
            ScreenState::Edit { buffer, .. }
            | ScreenState::Confirm { buffer, .. }
            | ScreenState::Busy { buffer, .. }
            | ScreenState::Result { buffer, .. } => buffer,
            ScreenState::Browse => return None,
        };
        let provider = buffer.provider?;
        let authentication = buffer.authentication?;
        let observed_context_limit = buffer
            .model
            .as_ref()
            .and_then(|model| {
                buffer
                    .discovered_models
                    .iter()
                    .find(|candidate| candidate.model == model.as_str())
            })
            .and_then(|candidate| candidate.context_limit);
        Some(ProviderEditCommand {
            profile_id: buffer.profile_id,
            name: buffer.name.clone(),
            provider,
            authentication,
            model: buffer.model.clone(),
            parameters: buffer.parameters.clone(),
            observed_context_limit,
        })
    }

    /// Replaces only committed, masked browse data after an operation completes. The reducer
    /// retains its edit buffer (without secret input) so a failure cannot make a local prediction
    /// look Active or erase fields that the user may retry.
    pub fn replace_browse(&mut self, browse: ProviderManagementView) {
        self.browse = browse;
    }

    pub fn apply_active_provider(&mut self, active: ActiveProviderView) {
        self.browse = ProviderManagementView::new(
            self.browse.catalog.clone(),
            self.browse.profiles.clone(),
            Some(active),
            self.browse.offline,
        );
    }

    pub fn start_create(&mut self, name: impl Into<String>) -> bool {
        if !matches!(self.state, ScreenState::Browse) {
            return false;
        }
        self.state = ScreenState::Edit {
            step: ProviderManagementStep::Provider,
            buffer: ProviderEditBuffer::new(None, name.into()),
        };
        true
    }

    pub fn start_edit(&mut self, profile: &ProviderProfileView) -> bool {
        if !matches!(self.state, ScreenState::Browse) {
            return false;
        }
        let mut buffer = ProviderEditBuffer::new(Some(profile.profile_id), profile.name.clone());
        buffer.provider = Some(profile.provider);
        buffer.model = Some(profile.model.clone());
        buffer.model_input = profile.model.as_str().to_owned();
        buffer.parameters = profile.parameters.clone();
        buffer.authentication = self
            .browse
            .catalog
            .iter()
            .find(|catalog| catalog.provider == profile.provider)
            .map(|catalog| catalog.credential_kind.into());
        buffer.has_saved_credential = profile.credential_status == CredentialViewStatus::Saved;
        self.state = ScreenState::Edit {
            step: ProviderManagementStep::Provider,
            buffer,
        };
        true
    }

    pub fn discard_edit(&mut self) -> bool {
        match self.state {
            ScreenState::Edit { .. } | ScreenState::Confirm { .. } | ScreenState::Result { .. } => {
                self.state = ScreenState::Browse;
                true
            }
            ScreenState::Browse | ScreenState::Busy { .. } => false,
        }
    }

    pub fn select_provider(&mut self, provider: ProviderId) -> bool {
        let ScreenState::Edit { step, buffer } = &mut self.state else {
            return false;
        };
        if *step != ProviderManagementStep::Provider {
            return false;
        }
        let provider_changed = buffer.provider != Some(provider);
        buffer.provider = Some(provider);
        buffer.model = None;
        buffer.model_input.clear();
        buffer.model_source = None;
        buffer.discovered_models.clear();
        buffer.model_filter.clear();
        buffer.highlighted_model = None;
        buffer.model_scroll = 0;
        buffer.parameter_issue = None;
        if provider_changed {
            buffer.has_saved_credential = false;
        }
        self.oauth_authorization = None;
        true
    }

    pub fn set_oauth_authorization(&mut self, authorization: Option<DeviceAuthorizationView>) {
        self.oauth_authorization = authorization;
    }

    pub fn select_authentication(&mut self, authentication: ProviderAuthentication) -> bool {
        let ScreenState::Edit { step, buffer } = &mut self.state else {
            return false;
        };
        if *step != ProviderManagementStep::Authentication {
            return false;
        }
        buffer.authentication = Some(authentication);
        true
    }

    /// Stores the input only in a `Zeroizing` buffer. Rendering exposes the constant mask, and
    /// beginning any external operation drops this input instead of retaining it for retries.
    pub fn set_secret_input(&mut self, value: &str) -> bool {
        let ScreenState::Edit { step, buffer } = &mut self.state else {
            return false;
        };
        if *step != ProviderManagementStep::Authentication
            || value.is_empty()
            || value
                .chars()
                .any(|character| !character.is_ascii() || character.is_ascii_control())
        {
            return false;
        }
        buffer.credential = Zeroizing::new(value.to_owned());
        true
    }

    /// Appends one keyboard character to the zeroizing API-key buffer. The renderer still sees
    /// only a fixed mask; this is deliberately not a general-purpose text or clipboard API.
    pub fn append_secret_character(&mut self, character: char) -> bool {
        let ScreenState::Edit { step, buffer } = &mut self.state else {
            return false;
        };
        if *step != ProviderManagementStep::Authentication
            || buffer.authentication != Some(ProviderAuthentication::ApiKey)
            || !character.is_ascii()
            || character.is_ascii_control()
        {
            return false;
        }
        buffer.credential.push(character);
        true
    }

    pub fn append_secret_text(&mut self, mut value: String) -> bool {
        let ScreenState::Edit { step, buffer } = &mut self.state else {
            value.zeroize();
            return false;
        };
        if *step != ProviderManagementStep::Authentication
            || buffer.authentication != Some(ProviderAuthentication::ApiKey)
        {
            value.zeroize();
            return false;
        }
        if value.is_empty()
            || value
                .chars()
                .any(|character| !character.is_ascii() || character.is_ascii_control())
        {
            value.zeroize();
            return false;
        }
        if buffer.credential.is_empty() {
            buffer.credential = Zeroizing::new(value);
        } else {
            buffer.credential.push_str(&value);
            value.zeroize();
        }
        true
    }

    pub fn delete_secret_character(&mut self) -> bool {
        let ScreenState::Edit { step, buffer } = &mut self.state else {
            return false;
        };
        if *step != ProviderManagementStep::Authentication
            || buffer.authentication != Some(ProviderAuthentication::ApiKey)
        {
            return false;
        }
        buffer.credential.pop().is_some()
    }

    /// Moves the typed value into the core's non-cloneable, non-debuggable secret type for the
    /// immediate service command. This is deliberately a move-only hand-off, not a reveal or a
    /// copy API; the rendering buffer is left empty before any asynchronous operation begins.
    pub fn take_secret_input(&mut self) -> Option<SecretValue> {
        let ScreenState::Edit { buffer, .. } = &mut self.state else {
            return None;
        };
        if buffer.credential.is_empty() {
            return None;
        }
        Some(SecretValue::from_utf8(std::mem::take(
            &mut *buffer.credential,
        )))
    }

    pub fn set_discovered_models(&mut self, models: Vec<DiscoveredModel>) -> bool {
        let ScreenState::Edit { step, buffer } = &mut self.state else {
            return false;
        };
        if *step != ProviderManagementStep::Model {
            return false;
        }
        replace_discovered_models(buffer, models);
        true
    }

    pub fn select_discovered_model(&mut self, model: &str) -> Result<(), ProviderEditIssue> {
        let ScreenState::Edit { step, buffer } = &mut self.state else {
            return Err(ProviderEditIssue::InvalidModel);
        };
        if *step != ProviderManagementStep::Model
            || !buffer
                .discovered_models
                .iter()
                .any(|candidate| candidate.model == model)
        {
            return Err(ProviderEditIssue::InvalidModel);
        }
        let provider = buffer.provider.ok_or(ProviderEditIssue::InvalidModel)?;
        let model = ProviderModelId::new(provider, model.to_owned())
            .map_err(|_| ProviderEditIssue::InvalidModel)?;
        buffer.model_input = model.as_str().to_owned();
        buffer.model = Some(model);
        buffer.model_source = Some(ProviderModelSource::Discovered);
        Ok(())
    }

    pub fn set_manual_model(&mut self, model: &str) -> Result<(), ProviderEditIssue> {
        let ScreenState::Edit { step, buffer } = &mut self.state else {
            return Err(ProviderEditIssue::InvalidModel);
        };
        if *step != ProviderManagementStep::Model {
            return Err(ProviderEditIssue::InvalidModel);
        }
        let provider = buffer.provider.ok_or(ProviderEditIssue::InvalidModel)?;
        let model = ProviderModelId::new(provider, model.to_owned())
            .map_err(|_| ProviderEditIssue::InvalidModel)?;
        buffer.model_input = model.as_str().to_owned();
        buffer.model = Some(model);
        buffer.model_source = Some(ProviderModelSource::Manual);
        Ok(())
    }

    /// Filters the online model list. Keyboard input never creates an arbitrary model ID.
    pub fn append_model_filter_character(&mut self, character: char) -> bool {
        let ScreenState::Edit { step, buffer } = &mut self.state else {
            return false;
        };
        if *step != ProviderManagementStep::Model {
            return false;
        }
        buffer.model_filter.push(character);
        reset_filtered_model_cursor(buffer);
        true
    }

    pub fn delete_model_filter_character(&mut self) -> bool {
        let ScreenState::Edit { step, buffer } = &mut self.state else {
            return false;
        };
        if *step != ProviderManagementStep::Model || buffer.model_filter.pop().is_none() {
            return false;
        }
        reset_filtered_model_cursor(buffer);
        true
    }

    pub fn append_model_filter_text(&mut self, value: &str) -> bool {
        let ScreenState::Edit { step, buffer } = &mut self.state else {
            return false;
        };
        if *step != ProviderManagementStep::Model {
            return false;
        }
        let filtered = value
            .chars()
            .filter(|character| !character.is_control())
            .collect::<String>();
        if filtered.is_empty() {
            return false;
        }
        buffer.model_filter.push_str(&filtered);
        reset_filtered_model_cursor(buffer);
        true
    }

    pub fn clear_model_filter(&mut self) -> bool {
        let ScreenState::Edit { step, buffer } = &mut self.state else {
            return false;
        };
        if *step != ProviderManagementStep::Model || buffer.model_filter.is_empty() {
            return false;
        }
        buffer.model_filter.clear();
        reset_filtered_model_cursor(buffer);
        true
    }

    pub fn select_highlighted_model(&mut self) -> bool {
        let ScreenState::Edit {
            step: ProviderManagementStep::Model,
            buffer,
        } = &mut self.state
        else {
            return false;
        };
        let Some(highlighted) = buffer.highlighted_model else {
            return false;
        };
        select_filtered_model(buffer, highlighted)
    }

    /// Rejects inapplicable parameters before replacing the buffer, so the widget can report the
    /// issue without silently dropping or forwarding a forbidden setting.
    pub fn set_parameters(
        &mut self,
        parameters: ProviderParameters,
        rules: &BTreeMap<ProviderParameterKey, ParameterApplicability>,
    ) -> Result<(), ProviderEditIssue> {
        let ScreenState::Edit { step, buffer } = &mut self.state else {
            return Err(ProviderEditIssue::UnsupportedParameters);
        };
        if *step != ProviderManagementStep::Parameters
            || parameters.validate_applicability(rules).is_err()
        {
            buffer.parameter_issue = Some(ProviderEditIssue::UnsupportedParameters);
            return Err(ProviderEditIssue::UnsupportedParameters);
        }
        buffer.parameters = parameters;
        buffer.parameter_issue = None;
        Ok(())
    }

    pub fn next_step(&mut self) -> bool {
        let ScreenState::Edit { step, buffer } = &mut self.state else {
            return false;
        };
        let complete = match *step {
            ProviderManagementStep::Provider => buffer.provider.is_some(),
            ProviderManagementStep::Authentication => buffer.authentication.is_some(),
            ProviderManagementStep::Model => buffer.model.is_some(),
            ProviderManagementStep::Parameters | ProviderManagementStep::Validate => true,
            ProviderManagementStep::SaveActivate => false,
        };
        if !complete {
            return false;
        }
        if let Some(next) = step.next() {
            *step = next;
            true
        } else {
            false
        }
    }

    pub fn previous_step(&mut self) -> bool {
        let ScreenState::Edit { step, .. } = &mut self.state else {
            return false;
        };
        if let Some(previous) = step.previous() {
            *step = previous;
            true
        } else {
            false
        }
    }

    pub fn request_discovery(&self) -> Option<ProviderScreenRequest> {
        self.request_at(
            ProviderManagementStep::Model,
            ProviderOperationKind::DiscoverModels,
        )
    }

    pub fn request_validation(&self) -> Option<ProviderScreenRequest> {
        self.request_at(
            ProviderManagementStep::Validate,
            ProviderOperationKind::Validate,
        )
    }

    pub fn request_oauth(&self) -> Option<ProviderScreenRequest> {
        self.request_at(
            ProviderManagementStep::Authentication,
            ProviderOperationKind::OAuth,
        )
    }

    pub fn request_save_draft(&self) -> Option<ProviderScreenRequest> {
        match self.state {
            ScreenState::Edit {
                step:
                    ProviderManagementStep::Authentication
                    | ProviderManagementStep::Validate
                    | ProviderManagementStep::SaveActivate,
                ..
            } => Some(ProviderScreenRequest::Operation(
                ProviderOperationKind::SaveDraft,
            )),
            _ => None,
        }
    }

    pub fn request_activation(&mut self) -> bool {
        let ScreenState::Edit { step, .. } = &self.state else {
            return false;
        };
        if *step != ProviderManagementStep::SaveActivate {
            return false;
        }
        let state = std::mem::replace(&mut self.state, ScreenState::Browse);
        let ScreenState::Edit { buffer, .. } = state else {
            unreachable!("state was matched before replacement");
        };
        self.state = ScreenState::Confirm {
            action: ProviderManagementAction::Activate,
            buffer,
        };
        true
    }

    pub fn cancel_confirmation(&mut self) -> bool {
        let state = std::mem::replace(&mut self.state, ScreenState::Browse);
        let ScreenState::Confirm { buffer, .. } = state else {
            self.state = state;
            return false;
        };
        self.state = ScreenState::Edit {
            step: ProviderManagementStep::SaveActivate,
            buffer,
        };
        true
    }

    pub fn confirm_activation(&self) -> Option<ProviderScreenRequest> {
        matches!(
            self.state,
            ScreenState::Confirm {
                action: ProviderManagementAction::Activate,
                ..
            }
        )
        .then_some(ProviderScreenRequest::Operation(
            ProviderOperationKind::Activate,
        ))
    }

    /// Transitions into Busy only for the operation that the current reducer state requested.
    /// The buffer is copied without its secret, shortening sensitive-memory lifetime before an
    /// asynchronous command is allowed to run.
    pub fn start_operation(
        &mut self,
        operation_id: OperationId,
        kind: ProviderOperationKind,
    ) -> bool {
        let allowed = match &self.state {
            ScreenState::Edit { step, .. } => match kind {
                ProviderOperationKind::DiscoverModels => *step == ProviderManagementStep::Model,
                ProviderOperationKind::Validate => *step == ProviderManagementStep::Validate,
                ProviderOperationKind::SaveDraft => {
                    matches!(
                        step,
                        ProviderManagementStep::Authentication
                            | ProviderManagementStep::Model
                            | ProviderManagementStep::Validate
                            | ProviderManagementStep::SaveActivate
                    )
                }
                ProviderOperationKind::Activate => false,
                ProviderOperationKind::OAuth => *step == ProviderManagementStep::Authentication,
            },
            ScreenState::Confirm { action, .. } => {
                *action == ProviderManagementAction::Activate
                    && kind == ProviderOperationKind::Activate
            }
            ScreenState::Browse | ScreenState::Busy { .. } | ScreenState::Result { .. } => false,
        };
        if !allowed {
            return false;
        }

        let state = std::mem::replace(&mut self.state, ScreenState::Browse);
        let (resume_step, buffer) = match state {
            ScreenState::Edit { step, buffer } => (step, buffer.safe_copy()),
            ScreenState::Confirm { buffer, .. } => {
                (ProviderManagementStep::SaveActivate, buffer.safe_copy())
            }
            _ => unreachable!("allowed state always owns an edit buffer"),
        };
        self.state = ScreenState::Busy {
            operation_id,
            kind,
            resume_step,
            buffer,
        };
        true
    }

    pub fn complete_discovery(
        &mut self,
        operation_id: OperationId,
        models: Vec<DiscoveredModel>,
    ) -> bool {
        let state = std::mem::replace(&mut self.state, ScreenState::Browse);
        let ScreenState::Busy {
            operation_id: current,
            kind: ProviderOperationKind::DiscoverModels,
            resume_step,
            mut buffer,
        } = state
        else {
            self.state = state;
            return false;
        };
        if current != operation_id {
            self.state = ScreenState::Busy {
                operation_id: current,
                kind: ProviderOperationKind::DiscoverModels,
                resume_step,
                buffer,
            };
            return false;
        }
        replace_discovered_models(&mut buffer, models);
        self.state = ScreenState::Edit {
            step: ProviderManagementStep::Model,
            buffer,
        };
        true
    }

    /// Confirms that online discovery still contains the exact persisted model selected for
    /// revalidation. A removed model must stop at the picker instead of silently validating a
    /// different candidate's context limit against the saved revision.
    pub fn selected_discovered_model_matches(&self, expected: &ProviderModelId) -> bool {
        let ScreenState::Edit {
            step: ProviderManagementStep::Model,
            buffer,
        } = &self.state
        else {
            return false;
        };
        buffer.model.as_ref() == Some(expected)
            && buffer.discovered_models.iter().any(|candidate| {
                candidate.model == expected.as_str()
                    && candidate.context_limit.is_some_and(|limit| limit > 0)
            })
    }

    pub fn move_discovered_model(&mut self, delta: isize) -> bool {
        let ScreenState::Edit {
            step: ProviderManagementStep::Model,
            buffer,
        } = &mut self.state
        else {
            return false;
        };
        let indices = filtered_model_indices(buffer);
        if indices.is_empty() {
            return false;
        }
        let current = buffer.highlighted_model.unwrap_or(0).min(indices.len() - 1);
        let selected = (current as isize + delta).rem_euclid(indices.len() as isize) as usize;
        buffer.highlighted_model = Some(selected);
        keep_model_highlight_visible(buffer);
        let _ = select_filtered_model(buffer, selected);
        true
    }

    pub fn move_discovered_model_page(&mut self, direction: isize) -> bool {
        let ScreenState::Edit {
            step: ProviderManagementStep::Model,
            buffer,
        } = &mut self.state
        else {
            return false;
        };
        let indices = filtered_model_indices(buffer);
        if indices.is_empty() {
            return false;
        }
        let current = buffer.highlighted_model.unwrap_or(0).min(indices.len() - 1);
        let selected = if direction < 0 {
            current.saturating_sub(VISIBLE_MODEL_ROWS)
        } else {
            current
                .saturating_add(VISIBLE_MODEL_ROWS)
                .min(indices.len() - 1)
        };
        buffer.highlighted_model = Some(selected);
        keep_model_highlight_visible(buffer);
        let _ = select_filtered_model(buffer, selected);
        true
    }

    pub fn complete_operation(
        &mut self,
        operation_id: OperationId,
        outcome: ProviderResultOutcome,
    ) -> bool {
        let state = std::mem::replace(&mut self.state, ScreenState::Browse);
        let ScreenState::Busy {
            operation_id: current,
            kind,
            resume_step,
            buffer,
        } = state
        else {
            self.state = state;
            return false;
        };
        if current != operation_id {
            self.state = ScreenState::Busy {
                operation_id: current,
                kind,
                resume_step,
                buffer,
            };
            return false;
        }
        if kind == ProviderOperationKind::Validate && outcome == ProviderResultOutcome::Succeeded {
            self.state = ScreenState::Edit {
                step: ProviderManagementStep::SaveActivate,
                buffer,
            };
            return true;
        }
        let resume_step = match &outcome {
            ProviderResultOutcome::Failed(error)
                if matches!(
                    error.field(),
                    Some(ProviderField::Credential | ProviderField::OAuth)
                ) =>
            {
                ProviderManagementStep::Authentication
            }
            _ => resume_step,
        };
        // A completion must never turn a failed/expired snapshot into active locally. The next
        // committed browse snapshot is supplied by the service integration task.
        self.state = ScreenState::Result {
            operation: kind,
            resume_step,
            outcome,
            buffer,
        };
        true
    }

    /// A durable draft is a precondition for compatibility validation. Updating only the opaque
    /// Profile identity is safe to retain in the reducer and lets the next Validate command read
    /// the revision through the service boundary; it does not copy a credential or imply active.
    pub fn complete_saved_draft(
        &mut self,
        operation_id: OperationId,
        profile_id: ProfileId,
        resume_step: ProviderManagementStep,
    ) -> bool {
        let state = std::mem::replace(&mut self.state, ScreenState::Browse);
        let ScreenState::Busy {
            operation_id: current,
            kind: ProviderOperationKind::SaveDraft,
            resume_step: requested_step,
            mut buffer,
        } = state
        else {
            self.state = state;
            return false;
        };
        if current != operation_id {
            self.state = ScreenState::Busy {
                operation_id: current,
                kind: ProviderOperationKind::SaveDraft,
                resume_step: requested_step,
                buffer,
            };
            return false;
        }
        buffer.profile_id = Some(profile_id);
        if buffer.authentication == Some(ProviderAuthentication::ApiKey) {
            buffer.has_saved_credential = true;
        }
        self.state = ScreenState::Edit {
            step: requested_step,
            buffer,
        };
        debug_assert_eq!(requested_step, resume_step);
        true
    }

    /// Records a failed Save Draft after its profile revision was durably created. Keeping the
    /// authoritative Profile identity makes a credential retry update that Profile instead of
    /// creating a duplicate.
    pub fn complete_partially_saved_draft(
        &mut self,
        operation_id: OperationId,
        profile_id: ProfileId,
        error: ProviderManagementError,
    ) -> bool {
        let state = std::mem::replace(&mut self.state, ScreenState::Browse);
        let ScreenState::Busy {
            operation_id: current,
            kind: ProviderOperationKind::SaveDraft,
            resume_step,
            mut buffer,
        } = state
        else {
            self.state = state;
            return false;
        };
        if current != operation_id {
            self.state = ScreenState::Busy {
                operation_id: current,
                kind: ProviderOperationKind::SaveDraft,
                resume_step,
                buffer,
            };
            return false;
        }
        buffer.profile_id = Some(profile_id);
        self.state = ScreenState::Result {
            operation: ProviderOperationKind::SaveDraft,
            resume_step,
            outcome: ProviderResultOutcome::Failed(error),
            buffer,
        };
        true
    }

    /// Returns the currently running operation for the event loop to cancel and restores only a
    /// non-sensitive edit buffer. A late completion is rejected by its no-longer-current ID.
    pub fn cancel_busy(&mut self) -> Option<OperationId> {
        let state = std::mem::replace(&mut self.state, ScreenState::Browse);
        let ScreenState::Busy {
            operation_id,
            resume_step,
            buffer,
            ..
        } = state
        else {
            self.state = state;
            return None;
        };
        self.state = ScreenState::Edit {
            step: resume_step,
            buffer,
        };
        Some(operation_id)
    }

    pub fn return_to_edit(&mut self) -> bool {
        let state = std::mem::replace(&mut self.state, ScreenState::Browse);
        let ScreenState::Result {
            resume_step,
            buffer,
            ..
        } = state
        else {
            self.state = state;
            return false;
        };
        self.state = ScreenState::Edit {
            step: resume_step,
            buffer,
        };
        true
    }

    pub fn retry(&mut self) -> Option<ProviderScreenRequest> {
        let state = std::mem::replace(&mut self.state, ScreenState::Browse);
        let ScreenState::Result {
            operation,
            resume_step,
            outcome: ProviderResultOutcome::Failed(error),
            buffer,
        } = state
        else {
            self.state = state;
            return None;
        };
        if !retry_is_safe(operation, resume_step, &buffer, &error) {
            self.state = ScreenState::Result {
                operation,
                resume_step,
                outcome: ProviderResultOutcome::Failed(error),
                buffer,
            };
            return None;
        }
        self.state = ScreenState::Edit {
            step: resume_step,
            buffer,
        };
        Some(ProviderScreenRequest::Operation(operation))
    }

    fn request_at(
        &self,
        step: ProviderManagementStep,
        operation: ProviderOperationKind,
    ) -> Option<ProviderScreenRequest> {
        matches!(
            self.state,
            ScreenState::Edit {
                step: current,
                ..
            } if current == step
        )
        .then_some(ProviderScreenRequest::Operation(operation))
    }
}

fn filtered_model_indices(buffer: &ProviderEditBuffer) -> Vec<usize> {
    let query = buffer.model_filter.trim().to_ascii_lowercase();
    buffer
        .discovered_models
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| {
            (query.is_empty() || candidate.model.to_ascii_lowercase().contains(&query))
                .then_some(index)
        })
        .collect()
}

fn select_filtered_model(buffer: &mut ProviderEditBuffer, filtered_index: usize) -> bool {
    let Some(candidate_index) = filtered_model_indices(buffer).get(filtered_index).copied() else {
        buffer.model = None;
        buffer.model_input.clear();
        buffer.model_source = None;
        return false;
    };
    let Some(provider) = buffer.provider else {
        return false;
    };
    if buffer.discovered_models[candidate_index]
        .context_limit
        .is_none_or(|limit| limit == 0)
    {
        buffer.model = None;
        buffer.model_input.clear();
        buffer.model_source = None;
        return false;
    }
    let Ok(model) = ProviderModelId::new(
        provider,
        buffer.discovered_models[candidate_index].model.clone(),
    ) else {
        return false;
    };
    buffer.model_input = model.as_str().to_owned();
    buffer.model = Some(model);
    buffer.model_source = Some(ProviderModelSource::Discovered);
    true
}

fn keep_model_highlight_visible(buffer: &mut ProviderEditBuffer) {
    let Some(highlighted) = buffer.highlighted_model else {
        buffer.model_scroll = 0;
        return;
    };
    if highlighted < buffer.model_scroll {
        buffer.model_scroll = highlighted;
    } else if highlighted >= buffer.model_scroll.saturating_add(VISIBLE_MODEL_ROWS) {
        buffer.model_scroll = highlighted + 1 - VISIBLE_MODEL_ROWS;
    }
}

fn reset_filtered_model_cursor(buffer: &mut ProviderEditBuffer) {
    buffer.model_scroll = 0;
    let filtered = filtered_model_indices(buffer);
    buffer.highlighted_model = filtered
        .iter()
        .position(|index| {
            buffer.discovered_models[*index]
                .context_limit
                .is_some_and(|limit| limit > 0)
        })
        .or_else(|| (!filtered.is_empty()).then_some(0));
    if buffer.highlighted_model.is_some() {
        let _ = select_filtered_model(
            buffer,
            buffer.highlighted_model.expect("highlight checked above"),
        );
    } else {
        buffer.model = None;
        buffer.model_input.clear();
        buffer.model_source = None;
    }
}

fn replace_discovered_models(buffer: &mut ProviderEditBuffer, models: Vec<DiscoveredModel>) {
    let current_model = buffer.model.as_ref().map(ProviderModelId::as_str);
    let current_index = current_model.and_then(|current| {
        models
            .iter()
            .position(|candidate| candidate.model == current)
    });
    buffer.discovered_models = models;
    buffer.model_filter.clear();
    buffer.model_scroll = 0;
    buffer.highlighted_model = current_index
        .or_else(|| {
            buffer
                .discovered_models
                .iter()
                .position(|candidate| candidate.context_limit.is_some_and(|limit| limit > 0))
        })
        .or_else(|| (!buffer.discovered_models.is_empty()).then_some(0));
    if let Some(highlighted) = buffer.highlighted_model {
        let _ = select_filtered_model(buffer, highlighted);
    } else {
        buffer.model = None;
        buffer.model_input.clear();
        buffer.model_source = None;
    }
}

fn retry_is_safe(
    operation: ProviderOperationKind,
    resume_step: ProviderManagementStep,
    buffer: &ProviderEditBuffer,
    error: &ProviderManagementError,
) -> bool {
    let retryable = error.remediation() == ProviderRemediation::Retry
        || error.retryability() == ys_agent_core::ProviderRetryability::Bounded;
    let consumed_api_key = operation == ProviderOperationKind::SaveDraft
        && resume_step == ProviderManagementStep::Authentication
        && buffer.authentication == Some(ProviderAuthentication::ApiKey);
    retryable && !consumed_api_key
}

#[cfg(test)]
mod tests {
    use super::*;
    use ys_agent_core::{CredentialKind, ProfileSummary, ProviderErrorCode, ProviderSupportStatus};

    fn profile_view(
        profile_id: ProfileId,
        provider: ProviderId,
        state: ProfileState,
        credential_status: CredentialViewStatus,
        oauth_status: Option<OAuthConnectionStatus>,
    ) -> ProviderProfileView {
        ProviderProfileView::from_detail(
            ProfileDetail {
                summary: ProfileSummary {
                    profile_id,
                    name: format!("{provider:?} profile"),
                    provider,
                    state,
                    credential_status,
                    is_active: true,
                },
                revision: 1,
                credential_generation: None,
                model: ProviderModelId::new(provider, format!("{}model", provider.model_prefix()))
                    .expect("test model uses provider prefix"),
                parameters: ProviderParameters::default(),
                validation_id: None,
                oauth_status,
            },
            None,
        )
    }

    fn catalog(provider: ProviderId, kind: CredentialKind) -> ProviderCatalogView {
        ProviderCatalogView {
            provider,
            display_name: format!("{provider:?}"),
            credential_kind: kind,
            support_status: ProviderSupportStatus::Candidate,
            evidence_gaps: Vec::new(),
        }
    }

    fn applicable_rules() -> BTreeMap<ProviderParameterKey, ParameterApplicability> {
        BTreeMap::from([
            (
                ProviderParameterKey::Temperature,
                ParameterApplicability::Supported,
            ),
            (
                ProviderParameterKey::Timeout,
                ParameterApplicability::Supported,
            ),
            (
                ProviderParameterKey::Retry,
                ParameterApplicability::Supported,
            ),
        ])
    }

    fn screen_at_model() -> ProviderManagementScreen {
        let mut screen = ProviderManagementScreen::new(ProviderManagementView::offline());
        assert!(screen.start_create("local DeepSeek"));
        assert!(screen.select_provider(ProviderId::DeepSeek));
        assert!(screen.next_step());
        assert!(screen.select_authentication(ProviderAuthentication::ApiKey));
        assert!(screen.next_step());
        screen
    }

    fn screen_at_validate() -> ProviderManagementScreen {
        let mut screen = screen_at_model();
        screen
            .set_manual_model("deepseek/reasoner")
            .expect("manual model is valid");
        assert!(screen.next_step());
        screen
            .set_parameters(ProviderParameters::default(), &applicable_rules())
            .expect("default parameters are applicable");
        assert!(screen.next_step());
        screen
    }

    fn screen_at_save_activate() -> ProviderManagementScreen {
        let mut screen = screen_at_validate();
        let operation_id = OperationId::new();
        assert_eq!(
            screen.request_validation(),
            Some(ProviderScreenRequest::Operation(
                ProviderOperationKind::Validate
            ))
        );
        assert!(screen.start_operation(operation_id, ProviderOperationKind::Validate));
        assert!(screen.complete_operation(operation_id, ProviderResultOutcome::Succeeded));
        screen
    }

    #[test]
    fn starts_in_an_offline_browse_state() {
        let screen = ProviderManagementScreen::new(ProviderManagementView::offline());

        assert_eq!(screen.state_kind(), ProviderManagementStateKind::Browse);
        assert!(screen.view().browse.offline);
    }

    #[test]
    fn selecting_a_provider_never_prefills_a_catalog_model() {
        let mut screen = ProviderManagementScreen::new(ProviderManagementView::new(
            vec![catalog(ProviderId::OpenCodeGo, CredentialKind::ApiKey)],
            Vec::new(),
            None,
            false,
        ));
        assert!(screen.start_create("OpenCode Go"));
        assert!(screen.select_provider(ProviderId::OpenCodeGo));

        let edit = screen.view().edit.expect("provider edit");
        assert_eq!(edit.model, None);
        assert_eq!(edit.model_source, None);
        assert!(edit.discovered_models.is_empty());
    }

    #[test]
    fn fixed_wizard_validates_before_distinct_save_and_activate_actions() {
        let mut screen = screen_at_save_activate();

        assert_eq!(screen.state_kind(), ProviderManagementStateKind::Edit);
        assert_eq!(
            screen.view().step,
            Some(ProviderManagementStep::SaveActivate)
        );
        assert_eq!(
            screen.request_save_draft(),
            Some(ProviderScreenRequest::Operation(
                ProviderOperationKind::SaveDraft
            ))
        );
        assert!(screen.request_activation());
        assert_eq!(screen.state_kind(), ProviderManagementStateKind::Confirm);
        assert_eq!(
            screen.confirm_activation(),
            Some(ProviderScreenRequest::Operation(
                ProviderOperationKind::Activate
            ))
        );
        assert_eq!(
            screen.view().confirmation,
            Some(ProviderConfirmationView {
                action: ProviderManagementAction::Activate,
                affects_new_runs_only: true,
            })
        );

        assert!(screen.cancel_confirmation());
        let save_id = OperationId::new();
        assert!(screen.start_operation(save_id, ProviderOperationKind::SaveDraft));
        assert!(screen.complete_operation(save_id, ProviderResultOutcome::Succeeded));
        assert_eq!(screen.state_kind(), ProviderManagementStateKind::Result);
        assert_eq!(
            screen.view().result.expect("save result").operation,
            ProviderOperationKind::SaveDraft
        );
    }

    #[test]
    fn discovered_and_manual_models_share_the_prefix_gate() {
        let mut screen = screen_at_model();
        assert!(screen.set_discovered_models(vec![DiscoveredModel {
            model: "deepseek/chat".to_owned(),
            context_limit: Some(32_000),
        }]));
        screen
            .select_discovered_model("deepseek/chat")
            .expect("approved discovered model");
        assert_eq!(
            screen.view().edit.expect("edit view").model_source,
            Some(ProviderModelSource::Discovered)
        );
        assert!(screen.set_manual_model("openai/not-allowed").is_err());
        screen
            .set_manual_model("deepseek/manual")
            .expect("manual model uses the same prefix gate");
        assert_eq!(
            screen.view().edit.expect("edit view").model_source,
            Some(ProviderModelSource::Manual)
        );
    }

    #[test]
    fn typing_filters_discovered_models_and_never_creates_an_arbitrary_model() {
        let mut screen = screen_at_model();
        assert!(screen.set_discovered_models(vec![
            DiscoveredModel {
                model: "deepseek/chat".to_owned(),
                context_limit: Some(32_000),
            },
            DiscoveredModel {
                model: "deepseek/reasoner".to_owned(),
                context_limit: Some(64_000),
            },
        ]));

        for character in "reason".chars() {
            assert!(screen.append_model_filter_character(character));
        }
        let edit = screen.view().edit.expect("filtered model view");
        assert_eq!(edit.model_filter, "reason");
        assert_eq!(edit.highlighted_model, Some(0));
        assert_eq!(
            edit.model
                .expect("first matching model is highlighted")
                .as_str(),
            "deepseek/reasoner"
        );
        assert!(screen.select_highlighted_model());

        for _ in 0.."reason".len() {
            assert!(screen.delete_model_filter_character());
        }
        assert_eq!(screen.view().edit.expect("cleared filter").model_filter, "");
    }

    #[test]
    fn pasted_filter_escape_and_page_navigation_follow_the_model_picker() {
        let mut screen = screen_at_model();
        assert!(
            screen.set_discovered_models(
                (0..20)
                    .map(|index| DiscoveredModel {
                        model: format!("deepseek/model-{index:02}"),
                        context_limit: Some(32_000),
                    })
                    .collect(),
            )
        );

        assert!(screen.move_discovered_model_page(1));
        assert_eq!(
            screen
                .view()
                .edit
                .expect("paged model view")
                .highlighted_model,
            Some(VISIBLE_MODEL_ROWS)
        );
        assert!(screen.move_discovered_model_page(-1));
        assert_eq!(
            screen.view().edit.expect("first page").highlighted_model,
            Some(0)
        );

        assert!(screen.append_model_filter_text("model-19\n"));
        let edit = screen.view().edit.expect("pasted filter");
        assert_eq!(edit.model_filter, "model-19");
        assert_eq!(
            edit.model.expect("matching model").as_str(),
            "deepseek/model-19"
        );
        assert!(screen.clear_model_filter());
        assert!(
            screen
                .view()
                .edit
                .expect("cleared filter")
                .model_filter
                .is_empty()
        );
    }

    #[test]
    fn a_model_without_context_evidence_is_visible_but_cannot_be_selected() {
        let mut screen = screen_at_model();
        assert!(screen.set_discovered_models(vec![DiscoveredModel {
            model: "deepseek/new-online-model".to_owned(),
            context_limit: None,
        }]));

        let edit = screen.view().edit.expect("online model remains visible");
        assert_eq!(edit.discovered_models.len(), 1);
        assert_eq!(edit.highlighted_model, Some(0));
        assert!(edit.model.is_none());
        assert!(!screen.select_highlighted_model());
        assert!(!screen.next_step());
    }

    #[test]
    fn discovery_initially_highlights_the_first_usable_model() {
        let mut screen = screen_at_model();
        assert!(screen.set_discovered_models(vec![
            DiscoveredModel {
                model: "deepseek/unknown".to_owned(),
                context_limit: None,
            },
            DiscoveredModel {
                model: "deepseek/ready-to-check".to_owned(),
                context_limit: Some(64_000),
            },
        ]));

        let edit = screen.view().edit.expect("discovered model list");
        assert_eq!(edit.highlighted_model, Some(1));
        assert_eq!(
            edit.model.expect("usable model selected").as_str(),
            "deepseek/ready-to-check"
        );
    }

    #[test]
    fn discovered_model_navigation_wraps_and_keeps_the_cursor_visible() {
        let mut screen = screen_at_model();
        assert!(
            screen.set_discovered_models(
                (0..12)
                    .map(|index| DiscoveredModel {
                        model: format!("deepseek/model-{index:02}"),
                        context_limit: Some(32_000),
                    })
                    .collect(),
            )
        );

        assert!(screen.move_discovered_model(-1));
        let edit = screen.view().edit.expect("wrapped selection");
        assert_eq!(edit.highlighted_model, Some(11));
        assert_eq!(edit.model_scroll, 3);
        assert_eq!(
            edit.model.expect("last model is selected").as_str(),
            "deepseek/model-11"
        );

        assert!(screen.move_discovered_model(1));
        let edit = screen.view().edit.expect("wrapped to first");
        assert_eq!(edit.highlighted_model, Some(0));
        assert_eq!(edit.model_scroll, 0);
    }

    #[test]
    fn unsupported_parameters_are_not_silently_replaced_or_forwarded() {
        let mut screen = screen_at_model();
        screen
            .set_manual_model("deepseek/reasoner")
            .expect("valid model");
        assert!(screen.next_step());

        let mut parameters = ProviderParameters::default();
        parameters
            .set_temperature(Some(0.4))
            .expect("finite test temperature");
        let mut unsupported = applicable_rules();
        unsupported.insert(
            ProviderParameterKey::Temperature,
            ParameterApplicability::Unsupported,
        );
        assert_eq!(
            screen.set_parameters(parameters.clone(), &unsupported),
            Err(ProviderEditIssue::UnsupportedParameters)
        );
        let rejected = screen.view().edit.expect("edit view");
        assert_eq!(rejected.parameters.temperature(), None);
        assert_eq!(
            rejected.parameter_issue,
            Some(ProviderEditIssue::UnsupportedParameters)
        );

        screen
            .set_parameters(parameters, &applicable_rules())
            .expect("supported value is retained");
        assert_eq!(
            screen
                .view()
                .edit
                .expect("edit view")
                .parameters
                .temperature(),
            Some(0.4)
        );
    }

    #[test]
    fn failures_and_cancel_keep_only_non_sensitive_edit_data() {
        let mut screen = screen_at_validate();
        assert!(screen.previous_step());
        assert_eq!(screen.view().step, Some(ProviderManagementStep::Parameters));
        assert!(screen.previous_step());
        assert_eq!(screen.view().step, Some(ProviderManagementStep::Model));
        assert!(screen.previous_step());
        assert!(screen.set_secret_input("secret-canary-must-never-render"));
        assert_eq!(
            screen.view().edit.expect("masked secret").credential_mask,
            Some(SECRET_MASK)
        );
        assert!(screen.next_step());
        assert!(screen.next_step());
        assert!(screen.next_step());

        let operation_id = OperationId::new();
        assert!(screen.start_operation(operation_id, ProviderOperationKind::Validate));
        let busy_rendered = format!("{:?}", screen.view());
        assert!(!busy_rendered.contains("secret-canary-must-never-render"));
        assert_eq!(
            screen.view().edit.expect("busy edit").credential_mask,
            None,
            "an asynchronous retry never retains typed secret input"
        );
        assert!(screen.complete_operation(
            operation_id,
            ProviderResultOutcome::Failed(ProviderManagementError::new(
                ProviderErrorCode::Network,
                None,
                ProviderRemediation::Retry,
            )),
        ));
        assert!(screen.view().result.expect("failure result").can_retry);
        assert_eq!(
            screen.retry(),
            Some(ProviderScreenRequest::Operation(
                ProviderOperationKind::Validate
            ))
        );
        assert_eq!(screen.view().step, Some(ProviderManagementStep::Validate));
        let retained = screen.view().edit.expect("safe retry buffer");
        assert_eq!(retained.name, "local DeepSeek");
        assert_eq!(retained.model.expect("model").as_str(), "deepseek/reasoner");
        assert_eq!(retained.credential_mask, None);
    }

    #[test]
    fn failed_api_key_save_returns_to_authentication_and_retains_persisted_profile_identity() {
        let mut screen = ProviderManagementScreen::new(ProviderManagementView::offline());
        assert!(screen.start_create("local DeepSeek"));
        assert!(screen.select_provider(ProviderId::DeepSeek));
        assert!(screen.next_step());
        assert!(screen.select_authentication(ProviderAuthentication::ApiKey));
        assert!(screen.set_secret_input("one-use-secret"));
        assert!(screen.take_secret_input().is_some());

        let operation_id = OperationId::new();
        assert!(screen.start_operation(operation_id, ProviderOperationKind::SaveDraft));
        let profile_id = ProfileId::new();
        assert!(screen.complete_partially_saved_draft(
            operation_id,
            profile_id,
            ProviderManagementError::new(
                ProviderErrorCode::Network,
                None,
                ProviderRemediation::Retry,
            ),
        ));

        let result = screen.view().result.expect("failed save result");
        assert!(!result.can_retry, "the consumed API key cannot be replayed");
        assert_eq!(screen.retry(), None);
        assert!(screen.return_to_edit());
        let view = screen.view();
        assert_eq!(view.step, Some(ProviderManagementStep::Authentication));
        assert_eq!(
            view.edit.expect("restored edit").profile_id,
            Some(profile_id)
        );
    }

    #[test]
    fn failed_save_from_validate_retries_from_validate() {
        let mut screen = screen_at_validate();
        let operation_id = OperationId::new();
        assert!(screen.start_operation(operation_id, ProviderOperationKind::SaveDraft));
        assert!(screen.complete_operation(
            operation_id,
            ProviderResultOutcome::Failed(ProviderManagementError::new(
                ProviderErrorCode::Network,
                None,
                ProviderRemediation::Retry,
            )),
        ));

        assert!(screen.view().result.expect("failed save result").can_retry);
        assert_eq!(
            screen.retry(),
            Some(ProviderScreenRequest::Operation(
                ProviderOperationKind::SaveDraft
            ))
        );
        assert_eq!(screen.view().step, Some(ProviderManagementStep::Validate));
    }

    #[test]
    fn failed_or_cancelled_model_save_returns_to_the_model_picker() {
        let mut failed = screen_at_model();
        failed
            .set_manual_model("deepseek/reasoner")
            .expect("selected model");
        let failed_id = OperationId::new();
        assert!(failed.start_operation(failed_id, ProviderOperationKind::SaveDraft));
        assert!(failed.complete_operation(
            failed_id,
            ProviderResultOutcome::Failed(ProviderManagementError::new(
                ProviderErrorCode::StorageConflict,
                None,
                ProviderRemediation::ReturnToEdit,
            )),
        ));
        assert!(failed.return_to_edit());
        assert_eq!(failed.view().step, Some(ProviderManagementStep::Model));

        let mut cancelled = screen_at_model();
        cancelled
            .set_manual_model("deepseek/reasoner")
            .expect("selected model");
        let cancelled_id = OperationId::new();
        assert!(cancelled.start_operation(cancelled_id, ProviderOperationKind::SaveDraft));
        assert_eq!(cancelled.cancel_busy(), Some(cancelled_id));
        assert_eq!(cancelled.view().step, Some(ProviderManagementStep::Model));
    }

    #[test]
    fn expired_saved_model_absent_from_discovery_requires_an_explicit_new_selection() {
        let profile_id = ProfileId::new();
        let profile = profile_view(
            profile_id,
            ProviderId::DeepSeek,
            ProfileState::Ready,
            CredentialViewStatus::Saved,
            None,
        );
        let expected = profile.model.clone();
        let browse = ProviderManagementView::new(
            vec![catalog(ProviderId::DeepSeek, CredentialKind::ApiKey)],
            vec![profile.clone()],
            None,
            false,
        );
        let mut screen = ProviderManagementScreen::new(browse);
        assert!(screen.start_edit(&profile));
        assert!(screen.next_step());
        assert!(screen.next_step());
        let operation_id = OperationId::new();
        assert!(screen.start_operation(operation_id, ProviderOperationKind::DiscoverModels));
        assert!(screen.complete_discovery(
            operation_id,
            vec![DiscoveredModel {
                model: "deepseek/replacement".to_owned(),
                context_limit: Some(32_768),
            }],
        ));

        assert!(!screen.selected_discovered_model_matches(&expected));
        assert_eq!(screen.view().step, Some(ProviderManagementStep::Model));
    }

    #[test]
    fn validation_credential_failure_returns_to_authentication() {
        let mut screen = screen_at_validate();
        let operation_id = OperationId::new();
        assert!(screen.start_operation(operation_id, ProviderOperationKind::Validate));
        assert!(screen.complete_operation(
            operation_id,
            ProviderResultOutcome::Failed(ProviderManagementError::new(
                ProviderErrorCode::CredentialMissing,
                Some(ProviderField::Credential),
                ProviderRemediation::ReturnToEdit,
            )),
        ));
        assert!(screen.return_to_edit());
        assert_eq!(
            screen.view().step,
            Some(ProviderManagementStep::Authentication)
        );
    }

    #[test]
    fn api_key_input_rejects_newlines_control_characters_and_non_ascii_text() {
        let mut screen = ProviderManagementScreen::new(ProviderManagementView::offline());
        assert!(screen.start_create("local DeepSeek"));
        assert!(screen.select_provider(ProviderId::DeepSeek));
        assert!(screen.next_step());
        assert!(screen.select_authentication(ProviderAuthentication::ApiKey));

        assert!(!screen.append_secret_text("abc\ndef".to_owned()));
        assert!(!screen.append_secret_character('\u{7f}'));
        assert!(!screen.append_secret_character('密'));
        assert_eq!(
            screen
                .view()
                .edit
                .expect("authentication edit")
                .credential_mask,
            None
        );
        assert!(screen.append_secret_text("sk-valid_ascii".to_owned()));
        assert_eq!(
            screen.view().edit.expect("masked API key").credential_mask,
            Some(SECRET_MASK)
        );
    }

    #[test]
    fn secret_handoff_is_move_only_and_oauth_cancel_returns_to_authentication() {
        let mut screen = ProviderManagementScreen::new(ProviderManagementView::offline());
        assert!(screen.start_create("subscription"));
        assert!(screen.select_provider(ProviderId::ChatGptSubscription));
        assert!(screen.next_step());
        assert!(screen.select_authentication(ProviderAuthentication::OAuth));
        assert!(screen.set_secret_input("oauth-canary-not-a-render-value"));
        let moved = screen
            .take_secret_input()
            .expect("typed secret moves to service boundary");
        assert_eq!(screen.view().edit.expect("edit view").credential_mask, None);
        drop(moved);

        assert_eq!(
            screen.request_oauth(),
            Some(ProviderScreenRequest::Operation(
                ProviderOperationKind::OAuth
            ))
        );
        let operation_id = OperationId::new();
        assert!(screen.start_operation(operation_id, ProviderOperationKind::OAuth));
        assert_eq!(screen.cancel_busy(), Some(operation_id));
        assert_eq!(
            screen.view().step,
            Some(ProviderManagementStep::Authentication)
        );
        assert!(
            !screen.complete_operation(operation_id, ProviderResultOutcome::Succeeded),
            "a cancelled operation may not apply its late result"
        );
    }

    #[test]
    fn canceling_an_edit_does_not_mutate_saved_or_active_browse_state() {
        let profile_id = ProfileId::new();
        let profile = profile_view(
            profile_id,
            ProviderId::DeepSeek,
            ProfileState::Ready,
            CredentialViewStatus::Saved,
            None,
        );
        let active = ActiveProviderView {
            activation_revision: 2,
            profile_id,
            profile_revision: 1,
            provider: ProviderId::DeepSeek,
            model: profile.model.clone(),
            parameters: profile.parameters.clone(),
        };
        let browse = ProviderManagementView::new(
            vec![catalog(ProviderId::DeepSeek, CredentialKind::ApiKey)],
            vec![profile.clone()],
            Some(active),
            true,
        );
        let original = browse.clone();
        let mut screen = ProviderManagementScreen::new(browse);

        assert!(screen.start_edit(&profile));
        let edit = screen.view().edit.expect("saved credential edit");
        assert_eq!(edit.credential_mask, Some(SECRET_MASK));
        assert!(edit.has_saved_credential);
        assert!(screen.discard_edit());
        assert_eq!(screen.state_kind(), ProviderManagementStateKind::Browse);
        assert_eq!(screen.view().browse, original);
        assert!(screen.view().browse.profiles[0].is_active);
    }

    #[test]
    fn expired_or_failed_revisions_never_render_as_active_and_offline_browse_keeps_profiles() {
        let expired_id = ProfileId::new();
        let failed_id = ProfileId::new();
        let expired = profile_view(
            expired_id,
            ProviderId::ChatGptSubscription,
            ProfileState::Ready,
            CredentialViewStatus::Expired,
            Some(OAuthConnectionStatus::Expired),
        );
        let failed = profile_view(
            failed_id,
            ProviderId::DeepSeek,
            ProfileState::Invalid,
            CredentialViewStatus::Saved,
            None,
        );
        let active = ActiveProviderView {
            activation_revision: 4,
            profile_id: expired_id,
            profile_revision: expired.revision,
            provider: expired.provider,
            model: expired.model.clone(),
            parameters: expired.parameters.clone(),
        };

        let view = ProviderManagementView::new(
            vec![
                catalog(
                    ProviderId::ChatGptSubscription,
                    CredentialKind::OAuthConnection,
                ),
                catalog(ProviderId::DeepSeek, CredentialKind::ApiKey),
            ],
            vec![expired, failed],
            Some(active),
            true,
        );
        assert!(view.offline);
        assert_eq!(view.profiles.len(), 2);
        assert!(view.active.is_none());
        assert!(view.profiles.iter().all(|profile| !profile.is_active));
    }
}
