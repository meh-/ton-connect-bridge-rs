use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct TonEvent {
    #[serde(skip_serializing_if = "String::is_empty")]
    #[serde(default = "default_id")]
    pub id: String,
    pub from: String,
    pub to: String,
    pub message: String,
    pub deadline: u64,
}

impl AsRef<str> for TonEvent {
    fn as_ref(&self) -> &str {
        &self.message
    }
}

fn default_id() -> String {
    "n/a".to_string()
}

impl TonEvent {
    pub fn serialize_for_sse(&self) -> Result<String, serde_json::Error> {
        #[derive(Serialize)]
        struct External<'a> {
            from: &'a String,
            message: &'a String,
        }

        let val = External {
            from: &self.from,
            message: &self.message,
        };
        serde_json::to_string(&val)
    }
}
