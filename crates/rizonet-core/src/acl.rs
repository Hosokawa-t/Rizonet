//! Capability-based access control for IPC commands.
//!
//! A capabilities policy is either:
//! - An `allow` list: only commands matching any of the patterns may run.
//! - A `deny` list: every command is allowed except those matching the patterns.
//! - Both: `allow` is applied first, then `deny` strips disallowed items.
//!
//! Patterns are simple glob-like strings using `*` as a single wildcard
//! matching any sequence of characters (no `?`, no `**`, no path semantics).
//! Examples: `"notes:*"`, `"*:read"`, `"dialog:open_file"`.
//!
//! When neither list is configured, all commands are allowed.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CapabilitiesConfig {
    /// When non-empty, only commands matching one of these patterns are permitted.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Commands matching one of these patterns are rejected.
    #[serde(default)]
    pub deny: Vec<String>,
}

impl CapabilitiesConfig {
    pub fn is_allowed(&self, cmd: &str) -> bool {
        let allow_ok = self.allow.is_empty() || self.allow.iter().any(|p| glob_match(p, cmd));
        if !allow_ok {
            return false;
        }
        !self.deny.iter().any(|p| glob_match(p, cmd))
    }
}

/// Minimal glob matcher: `*` matches any substring, everything else is literal.
fn glob_match(pattern: &str, text: &str) -> bool {
    // Split by '*' and require each piece to appear in order.
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut rest = text;
    // Anchor first piece to the start.
    let first = parts[0];
    if !rest.starts_with(first) {
        return false;
    }
    rest = &rest[first.len()..];

    if parts.len() == 1 {
        return rest.is_empty();
    }

    // Middle pieces can appear anywhere (in order).
    for piece in &parts[1..parts.len() - 1] {
        match rest.find(piece) {
            Some(idx) => {
                rest = &rest[idx + piece.len()..];
            }
            None => return false,
        }
    }

    // Anchor last piece to the end.
    let last = parts[parts.len() - 1];
    rest.ends_with(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_literal() {
        assert!(glob_match("ping", "ping"));
        assert!(!glob_match("ping", "pong"));
    }

    #[test]
    fn matches_wildcard() {
        assert!(glob_match("notes:*", "notes:list"));
        assert!(glob_match("*:read", "fs:read"));
        assert!(glob_match("a*b", "axxxb"));
        assert!(!glob_match("notes:*", "fs:list"));
    }

    #[test]
    fn allow_deny() {
        let c = CapabilitiesConfig {
            allow: vec!["notes:*".into(), "path:*".into()],
            deny: vec!["notes:delete".into()],
        };
        assert!(c.is_allowed("notes:list"));
        assert!(c.is_allowed("path:home"));
        assert!(!c.is_allowed("notes:delete"));
        assert!(!c.is_allowed("shell:open"));
    }
}
