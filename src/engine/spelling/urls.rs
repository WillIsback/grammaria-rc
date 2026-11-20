//! Tokens the spell checker never looks at: URLs and e-mail addresses.
//!
//! Port of the static `isUrl` and `isEMail` of
//! `org.languagetool.tokenizers.WordTokenizer` (LanguageTool,
//! LGPL-2.1-or-later), which `SpellingCheckRule` calls for every token.

use std::sync::{Arc, OnceLock};

use crate::re::{self, Flags, Regex};

/// `WordTokenizer.getProtocols`.
const PROTOCOLS: [&str; 18] = [
    "http", "https", "ws", "wss", "ftp", "ftps", "sftp", "file", "mailto", "tel", "sms", "git", "ssh", "data", "magnet", "smb", "slack", "spotify",
];

fn no_protocol_url() -> &'static Arc<Regex> {
    static PATTERN: OnceLock<Arc<Regex>> = OnceLock::new();
    PATTERN.get_or_init(|| {
        re::compile("([a-zA-Z0-9][a-zA-Z0-9-]+\\.)?([a-zA-Z0-9][a-zA-Z0-9-]+)\\.([a-zA-Z0-9][a-zA-Z0-9-]+)/.*", Flags::default()).expect("valid pattern")
    })
}

fn e_mail() -> &'static Arc<Regex> {
    static PATTERN: OnceLock<Arc<Regex>> = OnceLock::new();
    PATTERN.get_or_init(|| {
        re::compile(
            "(?<!:)@?\\b[a-zA-Z0-9.!#$%&'*+/=?^_`{|}~-]+@((\\[[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}\\])|(([a-zA-Z\\-0-9]+\\.)+[a-zA-Z]{2,}))\\b",
            Flags::default(),
        )
        .expect("valid pattern")
    })
}

/// `WordTokenizer.isUrl`.
pub fn is_url(token: &str) -> bool {
    if token.starts_with("www.") {
        return true;
    }
    if let Some(at) = token.find("://") {
        if PROTOCOLS.contains(&&token[..at]) {
            return true;
        }
    }
    // `NO_PROTOCOL_URL` needs a dot before a slash.
    match (token.find('.'), token.find('/')) {
        (Some(dot), Some(slash)) if dot < slash => no_protocol_url().is_full_match(token),
        _ => false,
    }
}

/// `WordTokenizer.isEMail`.
pub fn is_email(token: &str) -> bool {
    token.contains('@') && e_mail().is_full_match(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert!(is_url("https://example.org/a?b=c"));
        assert!(is_url("www.example.org"));
        assert!(is_url("example.org/page"));
        assert!(is_url("sub.example.org/"));
        assert!(!is_url("example.org"));
        assert!(!is_url("et/ou"));
        assert!(!is_url("xhttps://example.org"));
        assert!(!is_url("maison"));
    }

    #[test]
    fn e_mails() {
        assert!(is_email("jean.dupont@example.org"));
        assert!(!is_email("jean@"));
        assert!(!is_email("@jean"));
        assert!(!is_email("maison"));
    }
}
