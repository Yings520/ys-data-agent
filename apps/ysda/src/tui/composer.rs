use ratatui_textarea::{CursorMove, Input, Key, TextArea};

#[derive(Debug, Clone)]
pub struct ComposerState {
    textarea: TextArea<'static>,
    history: Vec<String>,
    history_index: Option<usize>,
    draft: Option<String>,
}

impl ComposerState {
    pub fn new() -> Self {
        let mut textarea = TextArea::default();
        textarea.set_max_histories(50);
        Self {
            textarea,
            history: Vec::new(),
            history_index: None,
            draft: None,
        }
    }

    pub fn text(&self) -> String {
        self.textarea.lines().join("\n")
    }

    pub fn set_text(&mut self, value: &str) {
        self.textarea = TextArea::from(value.split('\n'));
        self.textarea.set_max_histories(50);
        self.textarea.move_cursor(CursorMove::End);
        self.history_index = None;
    }

    pub fn textarea(&self) -> &TextArea<'static> {
        &self.textarea
    }
    pub fn textarea_mut(&mut self) -> &mut TextArea<'static> {
        &mut self.textarea
    }
    pub fn insert_paste(&mut self, value: &str) {
        self.textarea.insert_str(value);
    }
    pub fn handle_input(&mut self, input: Input) -> bool {
        self.history_index = None;
        self.textarea.input(input)
    }
    pub fn undo(&mut self) {
        self.textarea.undo();
    }
    pub fn redo(&mut self) {
        self.textarea.redo();
    }

    pub fn submit(&mut self) -> String {
        let value = self.text().trim().to_owned();
        if !value.is_empty() && self.history.last() != Some(&value) {
            self.history.push(value.clone());
        }
        self.set_text("");
        self.draft = None;
        value
    }

    pub fn history_up(&mut self) {
        if self.history.is_empty() {
            return;
        }
        if self.history_index.is_none() {
            self.draft = Some(self.text());
        }
        let index = self
            .history_index
            .map_or(self.history.len() - 1, |index| index.saturating_sub(1));
        self.history_index = Some(index);
        let value = self.history[index].clone();
        self.set_text(&value);
        self.history_index = Some(index);
    }

    pub fn history_down(&mut self) {
        let Some(index) = self.history_index else {
            return;
        };
        if index + 1 < self.history.len() {
            let next = index + 1;
            let value = self.history[next].clone();
            self.set_text(&value);
            self.history_index = Some(next);
        } else {
            let draft = self.draft.take().unwrap_or_default();
            self.set_text(&draft);
            self.history_index = None;
        }
    }

    pub fn clear(&mut self) {
        self.set_text("");
        self.draft = None;
    }
    pub fn insert_newline(&mut self) {
        let _ = self.textarea.input(Input {
            key: Key::Enter,
            ctrl: false,
            alt: false,
            shift: false,
        });
    }
}

impl Default for ComposerState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::ComposerState;
    use ratatui_textarea::{Input, Key};
    #[test]
    fn unicode_multiline_undo_redo_and_history_restore_the_draft() {
        let mut composer = ComposerState::new();
        composer.insert_paste("你好\nGMV");
        composer.handle_input(Input {
            key: Key::Char('!'),
            ctrl: false,
            alt: false,
            shift: false,
        });
        composer.undo();
        assert_eq!(composer.text(), "你好\nGMV");
        composer.redo();
        assert_eq!(composer.submit(), "你好\nGMV!");
        composer.set_text("draft");
        composer.history_up();
        assert_eq!(composer.text(), "你好\nGMV!");
        composer.history_down();
        assert_eq!(composer.text(), "draft");
    }
}
