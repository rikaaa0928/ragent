use openresponses_rust::Item;

/// Agent 对话上下文管理器。System Prompt 由 AgentDraft 单独管理。
#[derive(Debug, Clone, Default)]
pub struct AgentContext {
    items: Vec<Item>,
}

impl AgentContext {
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    pub fn from_existing(items: Vec<Item>) -> Self {
        Self {
            items: items
                .into_iter()
                .filter(|item| !is_system_message(item))
                .collect(),
        }
    }

    pub fn add_user_message(&mut self, message: impl Into<String>) {
        self.items.push(Item::user_message(message));
    }

    pub fn add_item(&mut self, item: Item) {
        if !is_system_message(&item) {
            self.items.push(item);
        }
    }

    pub fn add_items(&mut self, items: impl IntoIterator<Item = Item>) {
        self.items
            .extend(items.into_iter().filter(|item| !is_system_message(item)));
    }

    pub fn replace_items(&mut self, items: Vec<Item>) {
        self.items = items
            .into_iter()
            .filter(|item| !is_system_message(item))
            .collect();
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn to_items(&self) -> Vec<Item> {
        self.items.clone()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn has_user_input(&self) -> bool {
        !self.items.is_empty()
    }

    pub fn clear_history(&mut self) {
        self.items.clear();
    }
}

fn is_system_message(item: &Item) -> bool {
    match item {
        Item::Message { role, .. } => format!("{role:?}").eq_ignore_ascii_case("system"),
        _ => false,
    }
}
