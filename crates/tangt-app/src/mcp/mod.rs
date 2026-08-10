//! Model Context Protocol surface: the chart as tools an assistant can call.
//!
//! Two transports, one tool set (`tangt_core::tools`):
//! - `tangt mcp-serve` — stdio, for clients that start their own process
//!   ([`headless`]). Edits a `.tangt` file on disk.
//! - a loopback HTTP server the running app can switch on ([`runtime`]), so a
//!   client can drive the chart you are looking at.
//!
//! Both funnel every mutation through `Document::apply`, so an assistant's
//! edits are undoable with ⌘Z exactly like a canvas drag.

pub mod bridge;
pub mod headless;
pub mod runtime;
pub mod server;

use serde::{Deserialize, Serialize};

pub const DEFAULT_PORT: u16 = 8139;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct McpPrefs {
    /// Off by default: opening a port is a decision, not a default.
    pub server_enabled: bool,
    pub port: u16,
    /// Bearer token; regenerated on demand from the Preferences pane.
    pub token: String,
}

impl Default for McpPrefs {
    fn default() -> Self {
        McpPrefs {
            server_enabled: false,
            port: DEFAULT_PORT,
            token: new_token(),
        }
    }
}

impl McpPrefs {
    /// The block a user pastes into their MCP client's config.
    pub fn client_config(&self) -> String {
        format!(
            r#"{{
  "mcpServers": {{
    "tangt": {{
      "url": "http://127.0.0.1:{}/mcp",
      "headers": {{ "Authorization": "Bearer {}" }}
    }}
  }}
}}"#,
            self.port, self.token
        )
    }
}

pub fn new_token() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("system randomness is available");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Constant-time-ish comparison: never leak the token through timing.
pub fn token_matches(expected: &str, presented: &str) -> bool {
    if expected.len() != presented.len() {
        return false;
    }
    expected
        .bytes()
        .zip(presented.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_random_and_hex() {
        let a = new_token();
        let b = new_token();
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn token_comparison() {
        let t = new_token();
        assert!(token_matches(&t, &t));
        assert!(!token_matches(&t, "short"));
        assert!(!token_matches(&t, &"0".repeat(32)));
    }

    #[test]
    fn client_config_is_valid_json() {
        let prefs = McpPrefs::default();
        let parsed: serde_json::Value = serde_json::from_str(&prefs.client_config()).unwrap();
        assert_eq!(parsed["mcpServers"]["tangt"]["url"], "http://127.0.0.1:8139/mcp");
    }
}
