use openresponses_rust::{Item, MessageContent};

/// Agent 上下文管理器。System Prompt 单独保存并通过 API instructions 提交。
#[derive(Debug, Clone, Default)]
pub struct AgentContext {
    items: Vec<Item>,
    system_prompt: Option<String>,
}

impl AgentContext {
    pub fn new(system_prompt: Option<String>) -> Self {
        Self {
            items: Vec::new(),
            system_prompt,
        }
    }

    pub fn from_existing(items: Vec<Item>, system_prompt: Option<String>) -> Self {
        let system_prompt = system_prompt.or_else(|| items.iter().find_map(system_message_text));
        Self {
            items: items
                .into_iter()
                .filter(|item| !is_system_message(item))
                .collect(),
            system_prompt,
        }
    }

    pub fn set_system_prompt(&mut self, prompt: impl Into<String>) {
        self.system_prompt = Some(prompt.into());
    }

    pub fn system_prompt(&self) -> Option<&str> {
        self.system_prompt.as_deref()
    }

    pub fn has_system_prompt(&self) -> bool {
        self.system_prompt.is_some()
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

fn system_message_text(item: &Item) -> Option<String> {
    match item {
        Item::Message { role, content, .. }
            if format!("{role:?}").eq_ignore_ascii_case("system") =>
        {
            let text = content
                .iter()
                .filter_map(|part| match part {
                    MessageContent::InputText { text }
                    | MessageContent::PlainText { text }
                    | MessageContent::OutputText { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    }
}
