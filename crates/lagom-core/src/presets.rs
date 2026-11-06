use crate::account::{Security, ServerConfig};

pub fn servers(email: &str) -> Option<(ServerConfig, ServerConfig)> {
    let domain = email.rsplit_once('@')?.1.to_ascii_lowercase();
    let (imap, smtp) = match domain.as_str() {
        "gmail.com" | "googlemail.com" => ("imap.gmail.com", "smtp.gmail.com"),
        "icloud.com" | "me.com" | "mac.com" => ("imap.mail.me.com", "smtp.mail.me.com"),
        "fastmail.com" | "fastmail.fm" => ("imap.fastmail.com", "smtp.fastmail.com"),
        "outlook.com" | "hotmail.com" | "live.com" => {
            ("outlook.office365.com", "smtp-mail.outlook.com")
        }
        _ => return None,
    };
    Some((
        ServerConfig {
            host: imap.into(),
            port: 993,
            security: Security::Tls,
            username: email.into(),
        },
        ServerConfig {
            host: smtp.into(),
            port: 587,
            security: Security::StartTls,
            username: email.into(),
        },
    ))
}
pub fn is_gmail(account: &crate::account::Account) -> bool {
    account.imap.host.eq_ignore_ascii_case("imap.gmail.com")
}
#[cfg(test)]
mod tests {
    #[test]
    fn provider_defaults() {
        for email in [
            "a@GMAIL.COM",
            "a@icloud.com",
            "a@fastmail.com",
            "a@outlook.com",
        ] {
            let (imap, smtp) = super::servers(email).unwrap();
            assert_eq!(imap.port, 993);
            assert_eq!(smtp.port, 587);
            imap.validate().unwrap();
            smtp.validate().unwrap();
        }
        assert!(super::servers("a@custom.example").is_none());
    }
}
