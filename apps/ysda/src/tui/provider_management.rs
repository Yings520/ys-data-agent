//! Reserved for the Provider-management TUI reducer and view model (task 6.1).
//!
//! The screen remains I/O-free and reaches runtime behavior only through the
//! existing service API when its implementation is added.

use std::collections::BTreeMap;

use ys_agent_core::{
    ActiveProviderView, CompatibilityEvidenceView, CredentialKind, CredentialViewStatus,
    DiscoveredModel, OAuthConnectionStatus, OperationId, ParameterApplicability, ProfileDetail,
    ProfileId, ProfileState, ProviderCatalogView, ProviderId, ProviderManagementError,
    ProviderModelId, ProviderParameterKey, ProviderParameters, ProviderRemediation, SecretValue,
};
use zeroize::Zeroizing;

const SECRET_MASK: &str = "••••••••";

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
    pub parameters: ProviderParameters,
    pub credential_mask: Option<&'static str>,
    pub parameter_issue: Option<ProviderEditIssue>,
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
}

struct ProviderEditBuffer {
    profile_id: Option<ProfileId>,
    name: String,
    provider: Option<ProviderId>,
    authentication: Option<ProviderAuthentication>,
    model: Option<ProviderModelId>,
    model_source: Option<ProviderModelSource>,
    discovered_models: Vec<DiscoveredModel>,
    parameters: ProviderParameters,
    credential: Zeroizing<String>,
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
            model_source: None,
            discovered_models: Vec::new(),
            parameters: ProviderParameters::default(),
            credential: Zeroizing::new(String::new()),
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
            model_source: self.model_source,
            discovered_models: self.discovered_models.clone(),
            parameters: self.parameters.clone(),
            credential: Zeroizing::new(String::new()),
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
            parameters: self.parameters.clone(),
            credential_mask: (!self.credential.is_empty()).then_some(SECRET_MASK),
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
        buffer: ProviderEditBuffer,
    },
    Result {
        operation: ProviderOperationKind,
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
}

impl ProviderManagementScreen {
    pub fn new(browse: ProviderManagementView) -> Self {
        Self {
            browse,
            state: ScreenState::Browse,
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
            } => {
                view.edit = Some(buffer.view());
                view.busy = Some(ProviderBusyView {
                    operation_id: *operation_id,
                    kind: *kind,
                });
            }
            ScreenState::Result {
                operation,
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
                        error.remediation() == ProviderRemediation::Retry
                            || error.retryability() == ys_agent_core::ProviderRetryability::Bounded
                    }),
                });
            }
        }
        view
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
        buffer.parameters = profile.parameters.clone();
        buffer.authentication = self
            .browse
            .catalog
            .iter()
            .find(|catalog| catalog.provider == profile.provider)
            .map(|catalog| catalog.credential_kind.into());
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
        buffer.provider = Some(provider);
        buffer.model = None;
        buffer.model_source = None;
        buffer.discovered_models.clear();
        buffer.parameter_issue = None;
        true
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
        if *step != ProviderManagementStep::Authentication {
            return false;
        }
        buffer.credential = Zeroizing::new(value.to_owned());
        true
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
        buffer.discovered_models = models;
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
        buffer.model = Some(model);
        buffer.model_source = Some(ProviderModelSource::Manual);
        Ok(())
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
        self.request_at(
            ProviderManagementStep::SaveActivate,
            ProviderOperationKind::SaveDraft,
        )
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
                ProviderOperationKind::SaveDraft => *step == ProviderManagementStep::SaveActivate,
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
        let buffer = match state {
            ScreenState::Edit { buffer, .. } | ScreenState::Confirm { buffer, .. } => {
                buffer.safe_copy()
            }
            _ => unreachable!("allowed state always owns an edit buffer"),
        };
        self.state = ScreenState::Busy {
            operation_id,
            kind,
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
                buffer,
            };
            return false;
        }
        buffer.discovered_models = models;
        self.state = ScreenState::Edit {
            step: ProviderManagementStep::Model,
            buffer,
        };
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
        // A completion must never turn a failed/expired snapshot into active locally. The next
        // committed browse snapshot is supplied by the service integration task.
        self.state = ScreenState::Result {
            operation: kind,
            outcome,
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
            kind,
            buffer,
        } = state
        else {
            self.state = state;
            return None;
        };
        self.state = ScreenState::Edit {
            step: step_for_operation(kind),
            buffer,
        };
        Some(operation_id)
    }

    pub fn return_to_edit(&mut self) -> bool {
        let state = std::mem::replace(&mut self.state, ScreenState::Browse);
        let ScreenState::Result {
            operation, buffer, ..
        } = state
        else {
            self.state = state;
            return false;
        };
        self.state = ScreenState::Edit {
            step: step_for_operation(operation),
            buffer,
        };
        true
    }

    pub fn retry(&mut self) -> Option<ProviderScreenRequest> {
        let state = std::mem::replace(&mut self.state, ScreenState::Browse);
        let ScreenState::Result {
            operation,
            outcome: ProviderResultOutcome::Failed(error),
            buffer,
        } = state
        else {
            self.state = state;
            return None;
        };
        if error.remediation() != ProviderRemediation::Retry
            && error.retryability() != ys_agent_core::ProviderRetryability::Bounded
        {
            self.state = ScreenState::Result {
                operation,
                outcome: ProviderResultOutcome::Failed(error),
                buffer,
            };
            return None;
        }
        self.state = ScreenState::Edit {
            step: step_for_operation(operation),
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

fn step_for_operation(operation: ProviderOperationKind) -> ProviderManagementStep {
    match operation {
        ProviderOperationKind::DiscoverModels => ProviderManagementStep::Model,
        ProviderOperationKind::Validate => ProviderManagementStep::Validate,
        ProviderOperationKind::SaveDraft | ProviderOperationKind::Activate => {
            ProviderManagementStep::SaveActivate
        }
        ProviderOperationKind::OAuth => ProviderManagementStep::Authentication,
    }
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
