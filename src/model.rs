use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Vault {
    #[serde(default)]
    pub collections: Vec<Collection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Collection {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub tabs: Vec<Tab>,
}

impl Collection {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            tabs: vec![Tab::new("Tab 1")],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tab {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub groups: Vec<Group>,
}

impl Tab {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            groups: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub items: Vec<VaultItem>,
    #[serde(default = "default_group_color")]
    pub color: [u8; 3],
}

fn default_group_color() -> [u8; 3] {
    [180, 180, 180]
}

impl Group {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            items: Vec::new(),
            color: default_group_color(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum VaultItem {
    Text {
        id: Uuid,
        label: String,
        value: String,
        #[serde(default)]
        editing: bool,
        #[serde(default)]
        hidden: bool,
    },
    KeyValue {
        id: Uuid,
        label: String,
        key_label: String,
        value_label: String,
        key: String,
        value: String,
        #[serde(default)]
        editing: bool,
        #[serde(default)]
        hide_key: bool,
        #[serde(default)]
        hide_value: bool,
    },
    ReplaceText {
        id: Uuid,
        label: String,
        template: String,
        params: Vec<Param>,
        #[serde(default)]
        editing: bool,
        #[serde(default)]
        hidden: bool,
    },
    Note {
        id: Uuid,
        label: String,
        body: String,
        #[serde(default)]
        editing: bool,
    },
}

impl VaultItem {
    pub fn id(&self) -> Uuid {
        match self {
            VaultItem::Text { id, .. } => *id,
            VaultItem::KeyValue { id, .. } => *id,
            VaultItem::ReplaceText { id, .. } => *id,
            VaultItem::Note { id, .. } => *id,
        }
    }

    pub fn new_text() -> Self {
        VaultItem::Text {
            id: Uuid::new_v4(),
            label: "New text".into(),
            value: String::new(),
            editing: true,
            hidden: false,
        }
    }

    pub fn new_kv() -> Self {
        VaultItem::KeyValue {
            id: Uuid::new_v4(),
            label: "New entry".into(),
            key_label: "Key".into(),
            value_label: "Value".into(),
            key: String::new(),
            value: String::new(),
            editing: true,
            hide_key: false,
            hide_value: false,
        }
    }

    pub fn new_replace() -> Self {
        VaultItem::ReplaceText {
            id: Uuid::new_v4(),
            label: "New template".into(),
            template: String::new(),
            params: Vec::new(),
            editing: true,
            hidden: false,
        }
    }

    pub fn new_note() -> Self {
        VaultItem::Note {
            id: Uuid::new_v4(),
            label: "New note".into(),
            body: String::new(),
            editing: true,
        }
    }

    pub fn has_content(&self) -> bool {
        match self {
            VaultItem::Text { value, .. } => !value.is_empty(),
            VaultItem::KeyValue { key, value, .. } => !key.is_empty() || !value.is_empty(),
            VaultItem::ReplaceText { template, params, .. } => {
                !template.is_empty() || params.iter().any(|p| !p.value.is_empty())
            }
            VaultItem::Note { body, .. } => !body.is_empty(),
        }
    }
}

impl Group {
    pub fn has_content(&self) -> bool {
        self.items.iter().any(|i| i.has_content())
    }
}

impl Tab {
    pub fn has_content(&self) -> bool {
        self.groups.iter().any(|g| g.has_content())
    }
}

impl Collection {
    pub fn has_content(&self) -> bool {
        self.tabs.iter().any(|t| t.has_content())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    pub value: String,
}
