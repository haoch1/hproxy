use serde::{Deserialize, Serialize};

/// Missing bindings deliberately mean manual addresses, including old exports.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AddressBindings {
    #[serde(default)]
    pub public_host: bool,
    #[serde(default)]
    pub listen_ip: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_agent_id: Option<String>,
}
