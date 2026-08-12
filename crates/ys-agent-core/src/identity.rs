use crate::PrincipalId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]

pub enum Capability {
    DataQuery,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Principal {
    pub id: PrincipalId,
    pub display_name: String,
    pub capabilities: BTreeSet<Capability>,
}

impl Principal {
    pub fn local_operator(display_name: impl Into<String>) -> Self {
        let mut capabilities = BTreeSet::new();
        capabilities.insert(Capability::DataQuery);
        Self {
            id: PrincipalId::new(),
            display_name: display_name.into(),
            capabilities,
        }
    }

    pub fn has_capability(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }
}
