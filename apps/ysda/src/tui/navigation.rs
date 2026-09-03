#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ContentRoute {
    #[default]
    Timeline,
    Artifact,
    ModelSelection,
    ProviderManagement,
    Diagnostics,
}

/// Identifies one visit to a content route. Returning to the same route later produces a
/// different key, so work completed for the earlier visit cannot overwrite current state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct RouteKey {
    pub route: ContentRoute,
    pub generation: u64,
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
    generation: u64,
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
            generation: 0,
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

    pub fn route_key(&self) -> RouteKey {
        RouteKey {
            route: self.current(),
            generation: self.generation,
        }
    }

    pub fn push(&mut self, route: ContentRoute) {
        if route == ContentRoute::Timeline {
            if self.routes.len() > 1 {
                self.routes.truncate(1);
                self.generation = self.generation.wrapping_add(1);
            }
            return;
        }
        if self.current() != route {
            self.routes.push(route);
            self.generation = self.generation.wrapping_add(1);
        }
    }

    pub fn pop(&mut self) -> Option<ContentRoute> {
        let popped = (self.routes.len() > 1).then(|| self.routes.pop()).flatten();
        if popped.is_some() {
            self.generation = self.generation.wrapping_add(1);
        }
        popped
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
