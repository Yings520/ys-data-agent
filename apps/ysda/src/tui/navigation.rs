#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContentRoute {
    #[default]
    Timeline,
    Artifact,
    ModelSelection,
    ProviderManagement,
    Diagnostics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    CommandPalette,
    ModePicker,
    Help,
    Repair,
    ThemePicker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputLayer {
    Overlay,
    View,
    Composer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusTarget {
    #[default]
    Composer,
    TimelineResultCard,
    ArtifactContent,
    ModelSelectionList,
    ProviderManagement,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationState {
    routes: Vec<ContentRoute>,
    overlay: Option<Overlay>,
}

impl Default for NavigationState {
    fn default() -> Self {
        Self::new()
    }
}

impl NavigationState {
    pub fn new() -> Self {
        Self {
            routes: vec![ContentRoute::Timeline],
            overlay: None,
        }
    }

    pub fn routes(&self) -> &[ContentRoute] {
        &self.routes
    }

    pub fn current(&self) -> ContentRoute {
        self.routes
            .last()
            .copied()
            .unwrap_or(ContentRoute::Timeline)
    }

    pub fn push(&mut self, route: ContentRoute) {
        if route == ContentRoute::Timeline {
            self.routes.truncate(1);
            return;
        }
        if self.current() != route {
            self.routes.push(route);
        }
    }

    pub fn pop(&mut self) -> Option<ContentRoute> {
        (self.routes.len() > 1).then(|| self.routes.pop()).flatten()
    }

    pub fn open_overlay(&mut self, overlay: Overlay) -> bool {
        if self.overlay.is_some() {
            return false;
        }
        self.overlay = Some(overlay);
        true
    }

    pub fn overlay(&self) -> Option<Overlay> {
        self.overlay
    }

    pub fn close_overlay(&mut self) -> Option<Overlay> {
        self.overlay.take()
    }

    pub fn input_layer(&self) -> InputLayer {
        if self.overlay.is_some() {
            InputLayer::Overlay
        } else {
            InputLayer::View
        }
    }

    pub const fn input_priority() -> [InputLayer; 3] {
        [InputLayer::Overlay, InputLayer::View, InputLayer::Composer]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderNavigationState {
    pub search: String,
    pub highlighted: Option<usize>,
    pub scroll: usize,
    pub focus: FocusTarget,
}

impl Default for ProviderNavigationState {
    fn default() -> Self {
        Self {
            search: String::new(),
            highlighted: None,
            scroll: 0,
            focus: FocusTarget::ProviderManagement,
        }
    }
}
