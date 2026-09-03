#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKind {
    Mode,
    Model,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandSpec {
    pub kind: CommandKind,
    pub name: &'static str,
    pub description: &'static str,
}

impl CommandSpec {
    pub const fn new(name: &'static str, description: &'static str) -> Self {
        Self {
            kind: CommandKind::Model,
            name,
            description,
        }
    }

    const fn product(kind: CommandKind, name: &'static str, description: &'static str) -> Self {
        Self {
            kind,
            name,
            description,
        }
    }
}

const COMMAND_CATALOG: [CommandSpec; 2] = [
    CommandSpec::product(CommandKind::Mode, "mode", "choose Auto or Query mode"),
    CommandSpec::product(
        CommandKind::Model,
        "model",
        "choose active Provider and model",
    ),
];

pub fn command_catalog() -> &'static [CommandSpec] {
    &COMMAND_CATALOG
}

pub fn command_hint() -> String {
    COMMAND_CATALOG
        .iter()
        .map(|command| format!("/{}", command.name))
        .collect::<Vec<_>>()
        .join("  ")
}

pub trait SelectionItem {
    fn search_name(&self) -> &str;
    fn search_description(&self) -> &str;
}

impl SelectionItem for CommandSpec {
    fn search_name(&self) -> &str {
        self.name
    }

    fn search_description(&self) -> &str {
        self.description
    }
}

#[derive(Debug, Clone)]
pub struct Selector<T> {
    items: Vec<T>,
    matches: Vec<usize>,
    selected: usize,
    scroll: usize,
    visible_rows: usize,
    query: String,
    recompute_count: usize,
}

impl<T> Selector<T>
where
    T: SelectionItem + Clone + PartialEq,
{
    pub fn new(items: Vec<T>) -> Self {
        let mut selector = Self {
            items,
            matches: Vec::new(),
            selected: 0,
            scroll: 0,
            visible_rows: 6,
            query: String::new(),
            recompute_count: 0,
        };
        selector.recompute();
        selector
    }

    pub fn replace_items(&mut self, items: Vec<T>) {
        if self.items == items {
            return;
        }
        self.items = items;
        self.recompute();
    }

    pub fn update_query(&mut self, query: impl AsRef<str>) {
        let query = query.as_ref().to_ascii_lowercase();
        if self.query == query {
            return;
        }
        self.query = query;
        self.recompute();
    }

    pub fn matches(&self) -> impl Iterator<Item = &T> {
        self.matches.iter().map(|index| &self.items[*index])
    }

    pub fn rows(&self) -> impl Iterator<Item = (bool, &T)> {
        self.matches
            .iter()
            .skip(self.scroll)
            .take(self.visible_rows)
            .enumerate()
            .map(|(visible, index)| (self.scroll + visible == self.selected, &self.items[*index]))
    }

    pub fn selected(&self) -> Option<&T> {
        self.matches
            .get(self.selected)
            .map(|index| &self.items[*index])
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

    pub fn page_down(&mut self) {
        self.selected =
            (self.selected + self.visible_rows.max(1)).min(self.matches.len().saturating_sub(1));
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

    pub fn clear(&mut self) {
        self.matches.clear();
        self.selected = 0;
        self.scroll = 0;
        self.query.clear();
    }

    pub fn recompute_count(&self) -> usize {
        self.recompute_count
    }

    fn recompute(&mut self) {
        let mut matches = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| match_score(&self.query, item).map(|score| (score, index)))
            .collect::<Vec<_>>();
        matches.sort_by_key(|(score, index)| (*score, *index));
        self.matches = matches.into_iter().map(|(_, index)| index).collect();
        self.selected = 0;
        self.scroll = 0;
        self.recompute_count = self.recompute_count.saturating_add(1);
    }

    fn keep_selected_visible(&mut self) {
        if self.matches.is_empty() {
            self.selected = 0;
            self.scroll = 0;
            return;
        }
        if self.selected < self.scroll {
            self.scroll = self.selected;
        }
        let end = self.scroll.saturating_add(self.visible_rows);
        if self.selected >= end {
            self.scroll = self.selected + 1 - self.visible_rows;
        }
    }
}

fn match_score<T: SelectionItem>(query: &str, item: &T) -> Option<u8> {
    if query.is_empty() {
        return Some(4);
    }
    let name = item.search_name().to_ascii_lowercase();
    let description = item.search_description().to_ascii_lowercase();
    if name == query {
        Some(0)
    } else if name.starts_with(query) {
        Some(1)
    } else if is_ordered_subsequence(query, &name) {
        Some(2)
    } else if description.contains(query) {
        Some(3)
    } else {
        None
    }
}

fn is_ordered_subsequence(needle: &str, haystack: &str) -> bool {
    let mut characters = needle.chars();
    let mut next = characters.next();
    for candidate in haystack.chars() {
        if next == Some(candidate) {
            next = characters.next();
            if next.is_none() {
                return true;
            }
        }
    }
    next.is_none()
}

#[derive(Debug, Clone)]
pub struct SlashPalette {
    selector: Selector<CommandSpec>,
}

impl SlashPalette {
    pub fn with_default_commands() -> Self {
        Self {
            selector: Selector::new(command_catalog().to_vec()),
        }
    }

    pub fn update(&mut self, input: impl AsRef<str>) -> bool {
        let input = input.as_ref().trim_start();
        let Some(query) = input.strip_prefix('/') else {
            self.clear();
            return false;
        };
        self.selector.update_query(query);
        true
    }

    pub fn rows(&self) -> impl Iterator<Item = (bool, CommandSpec)> + '_ {
        self.selector
            .rows()
            .map(|(selected, item)| (selected, *item))
    }

    pub fn selected(&self) -> Option<CommandSpec> {
        self.selector.selected().copied()
    }

    pub fn move_up(&mut self) {
        self.selector.move_up();
    }

    pub fn move_down(&mut self) {
        self.selector.move_down();
    }

    pub fn page_up(&mut self) {
        self.selector.page_up();
    }

    pub fn page_down(&mut self) {
        self.selector.page_down();
    }

    pub fn select_visible_row(&mut self, row: usize) -> bool {
        self.selector.select_visible_row(row)
    }

    pub fn completion(&self) -> Option<(String, bool)> {
        self.selected()
            .map(|command| (format!("/{}", command.name), false))
    }

    pub fn clear(&mut self) {
        self.selector.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::{CommandSpec, Selector, SlashPalette};

    #[test]
    fn command_palette_has_one_catalog_and_keeps_no_match_open() {
        let mut palette = SlashPalette::with_default_commands();
        assert!(palette.update("/"));
        assert_eq!(
            palette
                .rows()
                .map(|(_, command)| command.name)
                .collect::<Vec<_>>(),
            vec!["mode", "model"]
        );
        assert_eq!(palette.rows().filter(|(selected, _)| *selected).count(), 1);
        palette.move_down();
        assert_eq!(palette.completion(), Some(("/model".to_owned(), false)));
        assert!(palette.update("/no-such-command"));
        assert!(palette.rows().next().is_none());
        assert_eq!(palette.selected(), None);
    }

    #[test]
    fn selector_ranks_exact_prefix_subsequence_and_description_stably() {
        let mut selector = Selector::new(vec![
            CommandSpec::new("xmode", "ordered characters"),
            CommandSpec::new("settings", "mode preferences"),
            CommandSpec::new("moderate", "first prefixed command"),
            CommandSpec::new("modeled", "prefixed command"),
            CommandSpec::new("mode", "choose execution behavior"),
        ]);

        selector.update_query("mode");
        assert_eq!(
            selector.matches().map(|item| item.name).collect::<Vec<_>>(),
            vec!["mode", "moderate", "modeled", "xmode", "settings"]
        );
    }

    #[test]
    fn selector_recomputes_only_for_query_or_candidate_changes() {
        let mut selector = Selector::new(vec![
            CommandSpec::new("mode", "choose execution behavior"),
            CommandSpec::new("model", "choose active provider model"),
        ]);
        let initial = selector.recompute_count();
        selector.update_query("mo");
        let searched = selector.recompute_count();
        assert!(searched > initial);
        selector.update_query("mo");
        selector.move_down();
        assert_eq!(selector.recompute_count(), searched);
        selector.replace_items(vec![CommandSpec::new(
            "model",
            "choose active provider model",
        )]);
        assert!(selector.recompute_count() > searched);
    }
}
