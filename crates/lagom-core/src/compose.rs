//! Shared reply and forwarding fields for native clients.
use crate::contacts::{recipient_address, split_recipients};

pub struct Original<'a> {
    pub from: &'a str, pub address: &'a str, pub date: &'a str,
    pub subject: &'a str, pub to: &'a str, pub text: &'a str,
}

/// "Re: subject", without stacking "Re: Re:".
pub fn reply_subject(subject: &str) -> String {
    if subject.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("re:")) { subject.into() } else { format!("Re: {subject}") }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ComposeKind { Reply, ReplyAll, Forward }

pub fn forward_subject(subject: &str) -> String {
    if subject.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("fwd:")) { subject.into() } else { format!("Fwd: {subject}") }
}

pub fn reply_all(sender: &str, to: &str, cc: &str, own: &[&str]) -> (String, String) {
    let mut used: std::collections::HashSet<String> = own.iter().map(|s| s.trim().to_ascii_lowercase()).collect();
    let mut unique = |field: &str| split_recipients(field).into_iter().filter(|address| used.insert(recipient_address(address).unwrap_or_else(|| address.clone()).to_ascii_lowercase())).collect::<Vec<_>>().join(", ");
    (unique(&format!("{sender}, {to}")), unique(cc))
}

pub fn compose_body(signature: &str, original: Option<Original<'_>>, kind: ComposeKind) -> String {
    let mut body = if signature.trim().is_empty() { String::new() } else { format!("-- \n{signature}") };
    if let Some(m) = original {
        if !body.is_empty() { body.push_str("\n\n"); }
        let header = if kind == ComposeKind::Forward {
            format!("Forwarded message\nFrom: {} <{}>\nDate: {}\nSubject: {}\nTo: {}\n\n", m.from, m.address, m.date, m.subject, m.to)
        } else { format!("On {}, {} wrote:\n\n", m.date, m.address) };
        body.push_str(&format!("{header}{}", m.text).lines().map(|line| format!("> {line}")).collect::<Vec<_>>().join("\n"));
    }
    if !body.is_empty() { body.insert_str(0, "\n\n"); }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_all_compares_mailboxes_and_keeps_display_names() {
        let (to, cc) = reply_all("Sender <sender@example.test>",
            "Me <ME@example.test>, sender@example.test, Other <other@example.test>",
            "Alias <alias@example.test>, OTHER@example.test, Copy <copy@example.test>",
            &["me@example.test", "alias@example.test"]);
        assert_eq!(to, "Sender <sender@example.test>, Other <other@example.test>");
        assert_eq!(cc, "Copy <copy@example.test>");
        assert_eq!(reply_all("me@example.test", "", "ME@example.test", &["me@example.test"]), (String::new(), String::new()));
    }

    #[test]
    fn quotes_and_signatures_share_desktop_format() {
        for kind in [ComposeKind::Reply, ComposeKind::ReplyAll, ComposeKind::Forward] {
            let body = compose_body("**Team**", Some(Original { from: "Sender", address: "sender@example.test",
                date: "2026-01-01 12:00", subject: "Plans", to: "me@example.test", text: "First\n\nSecond" }), kind);
            assert!(body.starts_with("\n\n-- \n**Team**\n\n> "));
            assert!(body.ends_with("> First\n> \n> Second"));
            assert_eq!(body.matches("-- \n**Team**").count(), 1);
            if kind == ComposeKind::Forward {
                assert!(body.contains("> Forwarded message\n> From: Sender <sender@example.test>\n> Date: "));
                assert!(body.contains("> Subject: Plans\n> To: me@example.test"));
            } else { assert!(body.contains("sender@example.test wrote:")); }
        }
        assert_eq!(compose_body("", None, ComposeKind::Reply), "");
        assert_eq!(reply_subject("RE: Plans"), "RE: Plans");
        assert_eq!(forward_subject("FWd: Plans"), "FWd: Plans");
        assert_eq!(reply_subject("日"), "Re: 日");
        assert_eq!(forward_subject("日"), "Fwd: 日");
    }
}
