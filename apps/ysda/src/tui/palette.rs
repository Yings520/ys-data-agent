#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub requires_arguments: bool,
}

#[derive(Debug, Clone)]
pub struct SlashPalette {
    commands: Vec<CommandSpec>,
    matches: Vec<usize>,
    selected: usize,
    scroll: usize,
    visible_rows: usize,
}

impl SlashPalette {
    pub fn with_default_commands() -> Self {
        Self::new(vec![
            CommandSpec {
                name: "metrics",
                description: "show key metrics",
                requires_arguments: false,
            },
            CommandSpec {
                name: "query",
                description: "show range and Query summary",
                requires_arguments: false,
            },
            CommandSpec {
                name: "checks",
                description: "show safety and governance checks",
                requires_arguments: false,
            },
            CommandSpec {
                name: "artifact",
                description: "show current or specified Artifact",
                requires_arguments: false,
            },
            CommandSpec {
                name: "sql",
                description: "show persisted executed SQL",
                requires_arguments: false,
            },
            CommandSpec {
                name: "details",
                description: "show Runtime diagnostics",
                requires_arguments: false,
            },
            CommandSpec {
                name: "theme",
                description: "choose or configure colors",
                requires_arguments: false,
            },
            CommandSpec {
                name: "new",
                description: "create a Session",
                requires_arguments: false,
            },
            CommandSpec {
                name: "tasks",
                description: "list Tasks in a focused view",
                requires_arguments: false,
            },
            CommandSpec {
                name: "task",
                description: "create a Task",
                requires_arguments: true,
            },
            CommandSpec {
                name: "resume",
                description: "resume a Task",
                requires_arguments: true,
            },
            CommandSpec {
                name: "cancel",
                description: "cancel a Run explicitly",
                requires_arguments: true,
            },
            CommandSpec {
                name: "export",
                description: "export a persisted Artifact",
                requires_arguments: true,
            },
            CommandSpec {
                name: "doctor",
                description: "rerun readiness checks",
                requires_arguments: false,
            },
            CommandSpec {
                name: "connections",
                description: "show source capabilities",
                requires_arguments: false,
            },
            CommandSpec {
                name: "providers",
                description: "manage Provider Profiles",
                requires_arguments: false,
            },
            CommandSpec {
                name: "model",
                description: "show provider and model",
                requires_arguments: false,
            },
            CommandSpec {
                name: "help",
                description: "show command help",
                requires_arguments: false,
            },
            CommandSpec {
                name: "quit",
                description: "detach the TUI",
                requires_arguments: false,
            },
        ])
    }
    pub fn new(commands: Vec<CommandSpec>) -> Self {
        Self {
            commands,
            matches: Vec::new(),
            selected: 0,
            scroll: 0,
            visible_rows: 6,
        }
    }
    pub fn update(&mut self, input: impl AsRef<str>) -> bool {
        let Some(prefix) = input.as_ref().strip_prefix('/') else {
            self.clear();
            return false;
        };
        let filter = prefix.to_ascii_lowercase();
        self.matches = self
            .commands
            .iter()
            .enumerate()
            .filter_map(|(index, command)| {
                let haystack =
                    format!("{} {}", command.name, command.description).to_ascii_lowercase();
                filter
                    .split_whitespace()
                    .all(|token| haystack.contains(token))
                    .then_some(index)
            })
            .collect();
        self.selected = self.selected.min(self.matches.len().saturating_sub(1));
        self.keep_selected_visible();
        !self.matches.is_empty()
    }
    pub fn rows(&self) -> impl Iterator<Item = (bool, CommandSpec)> + '_ {
        self.matches
            .iter()
            .skip(self.scroll)
            .take(self.visible_rows)
            .enumerate()
            .map(|(visible, index)| {
                (
                    self.scroll + visible == self.selected,
                    self.commands[*index],
                )
            })
    }
    pub fn selected(&self) -> Option<CommandSpec> {
        self.matches
            .get(self.selected)
            .map(|index| self.commands[*index])
    }
    pub fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
        self.keep_selected_visible();
    }
    pub fn move_down(&mut self) {
        self.selected = (self.selected + 1).min(self.matches.len().saturating_sub(1));
        self.keep_selected_visible();
    }
    pub fn page_up(&mut self) {
        self.selected = self.selected.saturating_sub(self.visible_rows.max(1));
        self.keep_selected_visible();
    }

    pub fn select_visible_row(&mut self, row: usize) -> bool {
        let selected = self.scroll.saturating_add(row);
        if selected >= self.matches.len() {
            return false;
        }
        self.selected = selected;
        self.keep_selected_visible();
        true
    }
    pub fn page_down(&mut self) {
        self.selected =
            (self.selected + self.visible_rows.max(1)).min(self.matches.len().saturating_sub(1));
        self.keep_selected_visible();
    }
    pub fn completion(&self) -> Option<(String, bool)> {
        self.selected()
            .map(|command| (format!("/{}", command.name), command.requires_arguments))
    }
    pub fn clear(&mut self) {
        self.matches.clear();
        self.selected = 0;
        self.scroll = 0;
    }
    fn keep_selected_visible(&mut self) {
        if self.selected < self.scroll {
            self.scroll = self.selected;
        }
        let end = self.scroll.saturating_add(self.visible_rows);
        if self.selected >= end {
            self.scroll = self.selected + 1 - self.visible_rows;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SlashPalette;

    #[test]
    fn palette_filters_name_and_description_and_keeps_selection_visible() {
        let mut palette = SlashPalette::with_default_commands();
        assert!(palette.update("/persisted sql"));
        assert_eq!(palette.selected().expect("SQL command").name, "sql");
        assert!(palette.update("/"));
        palette.page_down();
        palette.move_down();
        assert!(palette.rows().any(|(selected, _)| selected));
        assert!(palette.rows().count() <= 6);
        assert!(!palette.update("/no-such-command"));
    }

    #[test]
    fn completion_distinguishes_immediate_and_argument_commands() {
        let mut palette = SlashPalette::with_default_commands();
        assert!(palette.update("/sql"));
        assert_eq!(palette.completion(), Some(("/sql".to_owned(), false)));
        assert!(palette.update("/resume"));
        assert_eq!(palette.completion(), Some(("/resume".to_owned(), true)));
    }
}
