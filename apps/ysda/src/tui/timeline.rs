use super::navigation::FocusTarget;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HitRegion {
    x: u16,
    y: u16,
    width: u16,
    height: u16,
}

impl HitRegion {
    pub const fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub const fn contains(self, x: u16, y: u16) -> bool {
        x >= self.x
            && y >= self.y
            && x < self.x.saturating_add(self.width)
            && y < self.y.saturating_add(self.height)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineState {
    pub extension_lines: Vec<String>,
    pub scroll: usize,
    pub focus: FocusTarget,
    pub result_card_hit_region: Option<HitRegion>,
}

impl Default for TimelineState {
    fn default() -> Self {
        Self {
            extension_lines: Vec::new(),
            scroll: 0,
            focus: FocusTarget::Composer,
            result_card_hit_region: None,
        }
    }
}

pub fn render_lines(state: &TimelineState) -> Vec<String> {
    state.extension_lines.clone()
}

impl TimelineState {
    pub fn focus_result_card(&mut self, hit_region: HitRegion) {
        self.focus = FocusTarget::TimelineResultCard;
        self.result_card_hit_region = Some(hit_region);
    }
}
