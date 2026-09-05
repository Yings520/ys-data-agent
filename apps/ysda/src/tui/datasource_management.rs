use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

use ys_agent_core::{
    CommandId, ConnectorDescriptor, DatabaseContext, DatasourceDetail, DatasourceName,
    DatasourceSelectionKind, DatasourceView, DatasourceWriteContext, DeleteDatasource,
    DeleteDatasourceDisposition, FieldId, FieldInput, FieldValue, OperationId, ProfileId,
    SaveDatasource, SecretEdit, SecretValue, SelectDatasource, ValidateDatasource, ValidationMode,
};

use super::{SelectionItem, Selector};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatasourceScreenState {
    Browse,
    ConnectorSelect,
    Edit,
    Actions,
    ConfirmDelete,
    Busy,
    Result,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatasourceAction {
    New,
    Actions,
    Edit,
    Validate,
    SetDefault,
    Delete,
    ConfirmDelete,
    Confirm,
    Back,
    Retry,
    MoveUp,
    MoveDown,
    PageUp,
    PageDown,
    Home,
    End,
    NextField,
    PreviousField,
    Insert(char),
    Backspace,
}

pub enum DatasourceRequest {
    Save(SaveDatasource),
    Validate(ValidateDatasource),
    Select(SelectDatasource),
    Delete(DeleteDatasource),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BrowseItem {
    profile_index: usize,
    name: String,
    connector: String,
}

impl SelectionItem for BrowseItem {
    fn search_name(&self) -> &str {
        &self.name
    }
    fn search_description(&self) -> &str {
        &self.connector
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConnectorItem {
    descriptor: ConnectorDescriptor,
}

impl SelectionItem for ConnectorItem {
    fn search_name(&self) -> &str {
        &self.descriptor.display_name
    }
    fn search_description(&self) -> &str {
        self.descriptor.adapter_id.as_str()
    }
}

pub struct DatasourceForm {
    pub profile_id: Option<ProfileId>,
    pub name: String,
    pub values: BTreeMap<FieldId, FieldValue>,
    descriptor: ConnectorDescriptor,
    head_revision: Option<std::num::NonZeroU64>,
    has_saved_secret: bool,
    secret: Zeroizing<String>,
    focus: usize,
}

impl DatasourceForm {
    fn new(descriptor: ConnectorDescriptor) -> Self {
        let values = descriptor
            .fields
            .iter()
            .filter_map(|field| {
                (!matches!(field.input, FieldInput::Secret))
                    .then(|| field.default.clone().map(|value| (field.id.clone(), value)))
                    .flatten()
            })
            .collect();
        Self {
            profile_id: None,
            name: String::new(),
            values,
            descriptor,
            head_revision: None,
            has_saved_secret: false,
            secret: Zeroizing::new(String::new()),
            focus: 0,
        }
    }

    fn from_detail(detail: &DatasourceDetail, descriptor: ConnectorDescriptor) -> Self {
        Self {
            profile_id: Some(detail.profile.profile_id),
            name: detail.profile.name.as_str().to_owned(),
            values: detail.revision.input().fields.clone(),
            descriptor,
            head_revision: Some(detail.profile.head_revision),
            has_saved_secret: detail.revision.input().credential.is_some(),
            secret: Zeroizing::new(String::new()),
            focus: 0,
        }
    }

    fn focused_field(&self) -> Option<&ys_agent_core::DatasourceField> {
        self.focus
            .checked_sub(1)
            .and_then(|index| self.descriptor.fields.get(index))
    }

    fn field_text(&self, id: &FieldId) -> String {
        match self.values.get(id) {
            Some(FieldValue::Text(value)) => value.clone(),
            Some(FieldValue::Integer(value)) => value.to_string(),
            Some(FieldValue::Boolean(value)) => value.to_string(),
            None => String::new(),
        }
    }

    fn set_focused_text(&mut self, text: String) {
        if self.focus == 0 {
            self.name = text;
            return;
        }
        let Some(field) = self.descriptor.fields.get(self.focus - 1) else {
            return;
        };
        if matches!(field.input, FieldInput::Secret) {
            self.secret = Zeroizing::new(text);
            return;
        }
        if text.is_empty() {
            self.values.remove(&field.id);
            return;
        }
        let value = match &field.input {
            FieldInput::Integer { .. } => text.parse().ok().map(FieldValue::Integer),
            FieldInput::Boolean => text.parse().ok().map(FieldValue::Boolean),
            _ => Some(FieldValue::Text(text)),
        };
        if let Some(value) = value {
            self.values.insert(field.id.clone(), value);
        }
    }

    fn focused_text(&self) -> String {
        if self.focus == 0 {
            return self.name.clone();
        }
        let Some(field) = self.focused_field() else {
            return String::new();
        };
        if matches!(field.input, FieldInput::Secret) {
            self.secret.to_string()
        } else {
            self.field_text(&field.id)
        }
    }
}

pub struct DatasourceScreen {
    view: DatasourceView,
    state: DatasourceScreenState,
    browse: Selector<BrowseItem>,
    connectors: Selector<ConnectorItem>,
    form: Option<DatasourceForm>,
    selected_profile: Option<usize>,
    search: String,
    result: Option<String>,
    retry: Option<RetryKind>,
    result_back: DatasourceScreenState,
}

#[derive(Debug, Clone, Copy)]
enum RetryKind {
    Validate,
    Select(DatasourceSelectionKind),
    Delete,
}

impl DatasourceScreen {
    pub fn new(view: DatasourceView) -> Self {
        let browse = browse_selector(&view);
        let connectors = Selector::new(
            view.catalog
                .iter()
                .cloned()
                .map(|descriptor| ConnectorItem { descriptor })
                .collect(),
        );
        Self {
            view,
            state: DatasourceScreenState::Browse,
            browse,
            connectors,
            form: None,
            selected_profile: None,
            search: String::new(),
            result: None,
            retry: None,
            result_back: DatasourceScreenState::Browse,
        }
    }

    pub fn state(&self) -> DatasourceScreenState {
        self.state
    }
    pub fn form(&self) -> Option<&DatasourceForm> {
        self.form.as_ref()
    }
    pub fn view(&self) -> &DatasourceView {
        &self.view
    }
    pub fn highlighted_count(&self) -> usize {
        match self.state {
            DatasourceScreenState::ConnectorSelect => {
                usize::from(self.connectors.selected().is_some())
            }
            DatasourceScreenState::Browse => usize::from(self.browse.selected().is_some()),
            _ => 0,
        }
    }

    pub fn replace_view(&mut self, view: DatasourceView) {
        self.view = view;
        self.browse = browse_selector(&self.view);
        self.browse.update_query(&self.search);
    }

    pub fn select_profile(&mut self, profile_id: ProfileId) {
        self.selected_profile = self
            .view
            .snapshot
            .profiles
            .iter()
            .position(|detail| detail.profile.profile_id == profile_id);
    }

    pub fn complete(&mut self, view: DatasourceView, message: impl Into<String>) {
        self.replace_view(view);
        self.state = DatasourceScreenState::Result;
        self.result = Some(message.into());
        self.result_back = DatasourceScreenState::Browse;
    }

    pub fn fail(&mut self, message: impl Into<String>) {
        self.state = DatasourceScreenState::Result;
        self.result = Some(message.into());
    }

    pub fn reduce(&mut self, action: DatasourceAction) -> Option<DatasourceRequest> {
        if self.state == DatasourceScreenState::Busy {
            return None;
        }
        match action {
            DatasourceAction::New if self.state == DatasourceScreenState::Browse => {
                self.state = DatasourceScreenState::ConnectorSelect;
            }
            DatasourceAction::Actions if self.state == DatasourceScreenState::Browse => {
                self.capture_selected();
                if self.selected_profile.is_some() {
                    self.state = DatasourceScreenState::Actions;
                }
            }
            DatasourceAction::Edit if self.state == DatasourceScreenState::Actions => {
                self.start_edit_selected();
            }
            DatasourceAction::Validate
                if matches!(
                    self.state,
                    DatasourceScreenState::Actions | DatasourceScreenState::Result
                ) =>
            {
                return self.validate_request();
            }
            DatasourceAction::SetDefault if self.state == DatasourceScreenState::Actions => {
                return self.select_request(DatasourceSelectionKind::WorkspaceDefault);
            }
            DatasourceAction::Delete if self.state == DatasourceScreenState::Actions => {
                self.state = DatasourceScreenState::ConfirmDelete
            }
            DatasourceAction::ConfirmDelete
                if self.state == DatasourceScreenState::ConfirmDelete =>
            {
                return self.delete_request();
            }
            DatasourceAction::Confirm => return self.confirm(),
            DatasourceAction::Retry if self.state == DatasourceScreenState::Result => {
                return match self.retry {
                    Some(RetryKind::Validate) => self.validate_request(),
                    Some(RetryKind::Select(kind)) => self.select_request(kind),
                    Some(RetryKind::Delete) => self.delete_request(),
                    None => None,
                };
            }
            DatasourceAction::Back => self.back(),
            DatasourceAction::MoveUp => self.active_selector(-1, false),
            DatasourceAction::MoveDown => self.active_selector(1, false),
            DatasourceAction::PageUp => self.active_selector(-1, true),
            DatasourceAction::PageDown => self.active_selector(1, true),
            DatasourceAction::Home => self.active_home_end(false),
            DatasourceAction::End => self.active_home_end(true),
            DatasourceAction::NextField if self.state == DatasourceScreenState::Edit => {
                if let Some(form) = &mut self.form {
                    form.focus = (form.focus + 1).min(form.descriptor.fields.len());
                }
            }
            DatasourceAction::PreviousField if self.state == DatasourceScreenState::Edit => {
                if let Some(form) = &mut self.form {
                    form.focus = form.focus.saturating_sub(1);
                }
            }
            DatasourceAction::Insert(character) => self.insert(character),
            DatasourceAction::Backspace => self.backspace(),
            _ => {}
        }
        None
    }

    fn confirm(&mut self) -> Option<DatasourceRequest> {
        match self.state {
            DatasourceScreenState::ConnectorSelect => {
                self.form = self
                    .connectors
                    .selected()
                    .map(|item| DatasourceForm::new(item.descriptor.clone()));
                self.state = DatasourceScreenState::Edit;
                None
            }
            DatasourceScreenState::Edit => {
                let at_end = self
                    .form
                    .as_ref()
                    .is_some_and(|form| form.focus == form.descriptor.fields.len());
                if at_end {
                    self.save_request()
                } else {
                    self.reduce(DatasourceAction::NextField)
                }
            }
            DatasourceScreenState::Browse => {
                self.capture_selected();
                let ready = self.selected_detail().is_some_and(|detail| {
                    matches!(detail.state, ys_agent_core::RevisionState::Ready)
                });
                if ready {
                    self.select_request(DatasourceSelectionKind::Session)
                } else {
                    self.state = DatasourceScreenState::Actions;
                    None
                }
            }
            DatasourceScreenState::Result => {
                self.state = DatasourceScreenState::Browse;
                self.result = None;
                None
            }
            _ => None,
        }
    }

    fn save_request(&mut self) -> Option<DatasourceRequest> {
        let form = self.form.as_mut()?;
        let name = match DatasourceName::new(form.name.clone()) {
            Ok(name) => name,
            Err(_) => {
                self.state = DatasourceScreenState::Result;
                self.result_back = DatasourceScreenState::Edit;
                self.result =
                    Some("Name is required and must not contain control characters.".into());
                return None;
            }
        };
        if normalize_existing_file_fields(form).is_err() {
            self.state = DatasourceScreenState::Result;
            self.result_back = DatasourceScreenState::Edit;
            self.result = Some(
                "Database file must already exist and resolve to a readable regular file.".into(),
            );
            return None;
        }
        let has_secret = form.has_saved_secret || !form.secret.is_empty();
        if let Some(issue) = ys_agent_core::validate_datasource_fields(
            &form.descriptor.fields,
            &form.values,
            has_secret,
            true,
        )
        .first()
        {
            let field = issue.field.as_str().to_owned();
            self.state = DatasourceScreenState::Result;
            self.result_back = DatasourceScreenState::Edit;
            self.result = Some(format!("Check required field: {field}."));
            return None;
        }
        let context = match context_for(form) {
            Some(context) => context,
            None => {
                self.state = DatasourceScreenState::Result;
                self.result_back = DatasourceScreenState::Edit;
                self.result = Some("Datasource target fields are incomplete.".into());
                return None;
            }
        };
        let secret = if form.secret.is_empty() {
            SecretEdit::Keep
        } else {
            SecretEdit::Replace(SecretValue::from_utf8(std::mem::take(&mut *form.secret)))
        };
        let request = SaveDatasource {
            write: DatasourceWriteContext {
                command_id: CommandId::new(),
                scope: self.view.snapshot.selection.scope,
                expected_version: self.view.snapshot.version,
                expected_head_revision: form.head_revision,
            },
            profile_id: form.profile_id,
            name,
            adapter_id: form.descriptor.adapter_id.clone(),
            adapter_version: form.descriptor.adapter_version.clone(),
            config_version: form.descriptor.config_version,
            fields: form.values.clone(),
            context,
            secret,
        };
        self.result_back = DatasourceScreenState::Edit;
        self.state = DatasourceScreenState::Busy;
        Some(DatasourceRequest::Save(request))
    }

    fn validate_request(&mut self) -> Option<DatasourceRequest> {
        let detail = self.selected_detail()?;
        let request = ValidateDatasource {
            write: self.write(Some(detail.profile.head_revision)),
            revision: detail.revision.identity(),
            mode: ValidationMode::Connection,
            operation_id: OperationId::new(),
        };
        self.retry = Some(RetryKind::Validate);
        self.result_back = DatasourceScreenState::Actions;
        self.state = DatasourceScreenState::Busy;
        Some(DatasourceRequest::Validate(request))
    }

    fn select_request(&mut self, kind: DatasourceSelectionKind) -> Option<DatasourceRequest> {
        let detail = self.selected_detail()?;
        let request = SelectDatasource {
            write: self.write(Some(detail.profile.head_revision)),
            revision: detail.revision.identity(),
            kind,
        };
        self.retry = Some(RetryKind::Select(kind));
        self.result_back = DatasourceScreenState::Browse;
        self.state = DatasourceScreenState::Busy;
        Some(DatasourceRequest::Select(request))
    }

    fn delete_request(&mut self) -> Option<DatasourceRequest> {
        let detail = self.selected_detail()?;
        let request = DeleteDatasource {
            write: self.write(Some(detail.profile.head_revision)),
            profile_id: detail.profile.profile_id,
            disposition: DeleteDatasourceDisposition::ConfirmUnconfigured,
        };
        self.retry = Some(RetryKind::Delete);
        self.result_back = DatasourceScreenState::Actions;
        self.state = DatasourceScreenState::Busy;
        Some(DatasourceRequest::Delete(request))
    }

    fn write(&self, head: Option<std::num::NonZeroU64>) -> DatasourceWriteContext {
        DatasourceWriteContext {
            command_id: CommandId::new(),
            scope: self.view.snapshot.selection.scope,
            expected_version: self.view.snapshot.version,
            expected_head_revision: head,
        }
    }

    fn capture_selected(&mut self) {
        self.selected_profile = self.browse.selected().map(|item| item.profile_index);
    }
    fn selected_detail(&self) -> Option<&DatasourceDetail> {
        self.selected_profile
            .and_then(|index| self.view.snapshot.profiles.get(index))
    }
    fn start_edit_selected(&mut self) {
        let Some(detail) = self.selected_detail().cloned() else {
            return;
        };
        let Some(descriptor) = self
            .view
            .catalog
            .iter()
            .find(|descriptor| {
                descriptor.adapter_id == detail.revision.input().adapter_id
                    && descriptor.adapter_version == detail.revision.input().adapter_version
            })
            .cloned()
        else {
            return;
        };
        self.form = Some(DatasourceForm::from_detail(&detail, descriptor));
        self.state = DatasourceScreenState::Edit;
    }

    fn back(&mut self) {
        self.state = match self.state {
            DatasourceScreenState::ConnectorSelect => DatasourceScreenState::Browse,
            DatasourceScreenState::Edit => {
                if self
                    .form
                    .as_ref()
                    .is_some_and(|form| form.profile_id.is_some())
                {
                    DatasourceScreenState::Actions
                } else {
                    DatasourceScreenState::ConnectorSelect
                }
            }
            DatasourceScreenState::Actions => DatasourceScreenState::Browse,
            DatasourceScreenState::Result => self.result_back,
            DatasourceScreenState::ConfirmDelete => DatasourceScreenState::Actions,
            other => other,
        };
    }

    fn active_selector(&mut self, direction: i8, page: bool) {
        let selector: &mut dyn SelectorMove = match self.state {
            DatasourceScreenState::Browse => &mut self.browse,
            DatasourceScreenState::ConnectorSelect => &mut self.connectors,
            _ => return,
        };
        match (direction, page) {
            (-1, false) => selector.up(),
            (1, false) => selector.down(),
            (-1, true) => selector.page_up(),
            (1, true) => selector.page_down(),
            _ => {}
        }
    }
    fn active_home_end(&mut self, end: bool) {
        let selector: &mut dyn SelectorMove = match self.state {
            DatasourceScreenState::Browse => &mut self.browse,
            DatasourceScreenState::ConnectorSelect => &mut self.connectors,
            _ => return,
        };
        if end { selector.end() } else { selector.home() }
    }
    fn insert(&mut self, character: char) {
        if self.state == DatasourceScreenState::Browse {
            self.search.push(character);
            self.browse.update_query(&self.search);
            return;
        }
        if self.state != DatasourceScreenState::Edit {
            return;
        }
        if let Some(form) = &mut self.form {
            let mut text = form.focused_text();
            text.push(character);
            form.set_focused_text(text);
        }
    }
    fn backspace(&mut self) {
        if self.state == DatasourceScreenState::Browse {
            self.search.pop();
            self.browse.update_query(&self.search);
            return;
        }
        if let Some(form) = &mut self.form {
            let mut text = form.focused_text();
            text.pop();
            form.set_focused_text(text);
        }
    }

    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec!["Datasource management".to_owned()];
        match self.state {
            DatasourceScreenState::Browse => {
                lines.push(format!("Search: {}", self.search));
                lines.push("N new · Enter select/fix · A actions · Esc back".into());
                for (selected, item) in self.browse.rows() {
                    let detail = &self.view.snapshot.profiles[item.profile_index];
                    let current = if self.view.snapshot.selection.current
                        == Some(detail.revision.identity())
                    {
                        " [current]"
                    } else {
                        ""
                    };
                    let default = if self.view.snapshot.selection.workspace_default
                        == Some(detail.revision.identity())
                    {
                        " [default]"
                    } else {
                        ""
                    };
                    lines.push(format!(
                        "{} {} · {} · {:?}{}{}",
                        if selected { ">" } else { " " },
                        item.name,
                        item.connector,
                        detail.state,
                        current,
                        default
                    ));
                }
                if self.view.snapshot.profiles.is_empty() {
                    lines.push("No saved profiles. Press n to add one.".into());
                }
            }
            DatasourceScreenState::ConnectorSelect => {
                lines.push("Choose connector · ↑/↓ PgUp/PgDn Home/End Enter · Esc".into());
                for (selected, item) in self.connectors.rows() {
                    lines.push(format!(
                        "{} {}",
                        if selected { ">" } else { " " },
                        item.descriptor.display_name
                    ));
                }
            }
            DatasourceScreenState::Edit => {
                lines.push("Configure · Tab/Shift-Tab fields · Enter next/save · Esc".into());
                if let Some(form) = &self.form {
                    lines.push(format!(
                        "{} Name: {}",
                        if form.focus == 0 { ">" } else { " " },
                        form.name
                    ));
                    for (index, field) in form.descriptor.fields.iter().enumerate() {
                        let value = if matches!(field.input, FieldInput::Secret) {
                            "•".repeat(form.secret.chars().count())
                        } else {
                            form.field_text(&field.id)
                        };
                        lines.push(format!(
                            "{} {}: {}",
                            if form.focus == index + 1 { ">" } else { " " },
                            field.label,
                            value
                        ));
                    }
                }
            }
            DatasourceScreenState::Actions => lines.extend([
                "Actions".into(),
                "e edit · v validate · w workspace default · d delete · Esc".into(),
            ]),
            DatasourceScreenState::ConfirmDelete => lines.extend([
                "Delete this profile?".into(),
                "D confirm · Esc cancel".into(),
            ]),
            DatasourceScreenState::Busy => {
                lines.extend(["Operation in progress…".into(), "Esc/Ctrl-C cancel".into()])
            }
            DatasourceScreenState::Result => lines.extend([
                self.result.clone().unwrap_or_else(|| "Done".into()),
                "Enter/Esc return · r retry · v validate".into(),
            ]),
        }
        lines
    }
}

fn normalize_existing_file_fields(form: &mut DatasourceForm) -> Result<(), ()> {
    let file_fields = form
        .descriptor
        .fields
        .iter()
        .filter(|field| matches!(field.input, FieldInput::ExistingFile))
        .map(|field| field.id.clone())
        .collect::<Vec<_>>();
    for field_id in file_fields {
        let Some(FieldValue::Text(path)) = form.values.get(&field_id).cloned() else {
            continue;
        };
        let canonical = std::fs::canonicalize(Path::new(&path)).map_err(|_| ())?;
        if !canonical.is_file() {
            return Err(());
        }
        let canonical = canonical.to_str().ok_or(())?.to_owned();
        form.values.insert(field_id, FieldValue::Text(canonical));
    }
    Ok(())
}

fn browse_selector(view: &DatasourceView) -> Selector<BrowseItem> {
    Selector::new(
        view.snapshot
            .profiles
            .iter()
            .enumerate()
            .map(|(profile_index, detail)| {
                let connector = view
                    .catalog
                    .iter()
                    .find(|descriptor| descriptor.adapter_id == detail.revision.input().adapter_id)
                    .map(|descriptor| descriptor.display_name.clone())
                    .unwrap_or_else(|| detail.revision.input().adapter_id.as_str().to_owned());
                BrowseItem {
                    profile_index,
                    name: detail.profile.name.as_str().to_owned(),
                    connector,
                }
            })
            .collect(),
    )
}

fn context_for(form: &DatasourceForm) -> Option<DatabaseContext> {
    let text = |name: &str| {
        form.values
            .get(&FieldId::new(name).ok()?)
            .and_then(|value| match value {
                FieldValue::Text(value) => Some(value.clone()),
                _ => None,
            })
    };
    match form.descriptor.adapter_id.as_str() {
        "sqlite" | "duckdb" => Some(DatabaseContext::File {
            canonical_path: PathBuf::from(text("database_path")?),
        }),
        "postgres" => {
            let host = text("host")?;
            let port =
                form.values
                    .get(&FieldId::new("port").ok()?)
                    .and_then(|value| match value {
                        FieldValue::Integer(value) => u16::try_from(*value).ok(),
                        _ => None,
                    })?;
            let database = text("database")?;
            let schema = text("schema")?;
            Some(DatabaseContext::Database {
                catalog: Some(format!("{host}:{port}")),
                database,
                schema,
            })
        }
        _ => None,
    }
}

trait SelectorMove {
    fn up(&mut self);
    fn down(&mut self);
    fn page_up(&mut self);
    fn page_down(&mut self);
    fn home(&mut self);
    fn end(&mut self);
}
impl<T: SelectionItem + Clone + PartialEq> SelectorMove for Selector<T> {
    fn up(&mut self) {
        self.move_up()
    }
    fn down(&mut self) {
        self.move_down()
    }
    fn page_up(&mut self) {
        self.page_up()
    }
    fn page_down(&mut self) {
        self.page_down()
    }
    fn home(&mut self) {
        self.home()
    }
    fn end(&mut self) {
        self.end()
    }
}
