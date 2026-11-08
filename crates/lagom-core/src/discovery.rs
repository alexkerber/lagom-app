//! Email-first discovery. MX uses the system DNS configuration; ISPDB receives
//! only the domain, never the email address or credentials.
use crate::{
    account::{Security, ServerConfig},
    oauth::{self, Provider},
};
use serde::Deserialize;
use std::{future::Future, time::Duration};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preset {
    pub name: String,
    pub imap: ServerConfig,
    pub smtp: ServerConfig,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Discovery {
    OAuth(Provider),
    Password {
        preset: Preset,
        help_url: Option<String>,
    },
    Manual,
}
const DEADLINE: Duration = Duration::from_secs(3);

pub async fn discover(email: &str) -> Discovery {
    discover_with(
        email,
        |domain| async move {
            let resolver = hickory_resolver::TokioAsyncResolver::tokio_from_system_conf().ok()?;
            let mx = resolver.mx_lookup(domain).await.ok()?;
            Some(mx.iter().map(|r| r.exchange().to_utf8()).collect())
        },
        |domain| async move {
            let response = oauth::http(DEADLINE)
                .ok()?
                .get(format!("https://autoconfig.thunderbird.net/v1.1/{domain}"))
                .send()
                .await
                .ok()?;
            if !response.status().is_success() {
                return None;
            }
            String::from_utf8(oauth::body(response).await.ok()?.to_vec()).ok()
        },
    )
    .await
}
async fn discover_with<M, MF, A, AF>(email: &str, mx: M, autoconfig: A) -> Discovery
where
    M: FnOnce(String) -> MF,
    MF: Future<Output = Option<Vec<String>>>,
    A: FnOnce(String) -> AF,
    AF: Future<Output = Option<String>>,
{
    if email.parse::<lettre::Address>().is_err()
        || email.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Discovery::Manual;
    }
    let Some((_, domain)) = email.rsplit_once('@') else {
        return Discovery::Manual;
    };
    let Ok(url::Host::Domain(domain)) = url::Host::parse(domain) else {
        return Discovery::Manual;
    };
    if domain.len() > 253
        || domain.split('.').any(|s| {
            s.is_empty()
                || s.len() > 63
                || s.starts_with('-')
                || s.ends_with('-')
                || !s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
    {
        return Discovery::Manual;
    }
    if let Some(result) = known(&domain, email) {
        return result;
    }
    if let Ok(Some(hosts)) = tokio::time::timeout(DEADLINE, mx(domain.clone())).await {
        if let Some(result) = hosts.iter().find_map(|host| mx_provider(host, email)) {
            return result;
        }
    }
    if let Ok(Some(xml)) = tokio::time::timeout(DEADLINE, autoconfig(domain)).await {
        if let Some(preset) = parse_autoconfig(&xml, email) {
            return Discovery::Password {
                preset,
                help_url: None,
            };
        }
    }
    Discovery::Manual
}
fn password(email: &str, icloud: bool) -> Discovery {
    let (name, imap, smtp, help) = if icloud {
        (
            "iCloud",
            "imap.mail.me.com",
            "smtp.mail.me.com",
            "https://account.apple.com/account/manage",
        )
    } else {
        (
            "Fastmail",
            "imap.fastmail.com",
            "smtp.fastmail.com",
            "https://app.fastmail.com/settings/security/apppasswords",
        )
    };
    Discovery::Password {
        preset: Preset {
            name: name.into(),
            imap: ServerConfig {
                host: imap.into(),
                port: 993,
                security: Security::Tls,
                username: email.into(),
            },
            smtp: ServerConfig {
                host: smtp.into(),
                port: 587,
                security: Security::StartTls,
                username: email.into(),
            },
        },
        help_url: Some(help.into()),
    }
}
fn known(domain: &str, email: &str) -> Option<Discovery> {
    Some(match domain {
        "gmail.com" | "googlemail.com" => Discovery::OAuth(Provider::Google),
        "outlook.com" | "msn.com" => Discovery::OAuth(Provider::Microsoft),
        "icloud.com" | "me.com" | "mac.com" => password(email, true),
        d if d.starts_with("hotmail.") || d.starts_with("live.") => {
            Discovery::OAuth(Provider::Microsoft)
        }
        d if d.starts_with("fastmail.") => password(email, false),
        _ => return None,
    })
}
fn mx_provider(host: &str, email: &str) -> Option<Discovery> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let under = |domain: &str| {
        host == domain || host.strip_suffix(domain).is_some_and(|s| s.ends_with('.'))
    };
    if under("google.com") || under("googlemail.com") {
        Some(Discovery::OAuth(Provider::Google))
    } else if under("mail.protection.outlook.com") {
        Some(Discovery::OAuth(Provider::Microsoft))
    } else if under("icloud.com") {
        Some(password(email, true))
    } else if under("messagingengine.com") {
        Some(password(email, false))
    } else {
        None
    }
}
#[derive(Deserialize)]
struct ClientConfig {
    #[serde(rename = "emailProvider")]
    provider: XmlProvider,
}
#[derive(Deserialize)]
struct XmlProvider {
    #[serde(rename = "displayName", default)]
    name: String,
    #[serde(rename = "incomingServer", default)]
    incoming: Vec<XmlServer>,
    #[serde(rename = "outgoingServer", default)]
    outgoing: Vec<XmlServer>,
}
#[derive(Deserialize)]
struct XmlServer {
    #[serde(rename = "@type")]
    kind: String,
    hostname: String,
    port: u16,
    #[serde(rename = "socketType")]
    security: String,
    username: String,
    #[serde(default)]
    authentication: Vec<String>,
}
fn parse_autoconfig(xml: &str, email: &str) -> Option<Preset> {
    let config: ClientConfig = quick_xml::de::from_str(xml).ok()?;
    let (local, domain) = email.rsplit_once('@')?;
    let expand = |value: &str| {
        value
            .replace("%EMAILADDRESS%", email)
            .replace("%EMAILLOCALPART%", local)
            .replace("%EMAILDOMAIN%", domain)
    };
    let server = |s: &XmlServer, kind: &str| {
        if s.kind != kind || !s.authentication.iter().any(|a| a == "password-cleartext") {
            return None;
        }
        let config = ServerConfig {
            host: expand(&s.hostname),
            port: s.port,
            security: match s.security.as_str() {
                "SSL" => Security::Tls,
                "STARTTLS" => Security::StartTls,
                _ => return None,
            },
            username: expand(&s.username),
        };
        if config.host.contains('%') || config.username.contains('%') || config.validate().is_err()
        {
            return None;
        }
        Some(config)
    };
    let name = if config.provider.name.is_empty() {
        domain.into()
    } else {
        config.provider.name
    };
    if name.chars().any(char::is_control) {
        return None;
    }
    Some(Preset {
        name,
        imap: config
            .provider
            .incoming
            .iter()
            .find_map(|s| server(s, "imap"))?,
        smtp: config
            .provider
            .outgoing
            .iter()
            .find_map(|s| server(s, "smtp"))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const XML: &str = r#"<clientConfig version="1.1"><emailProvider id="example.com"><displayName>Example</displayName>
        <incomingServer type="imap"><hostname>imap.%EMAILDOMAIN%</hostname><port>993</port><socketType>SSL</socketType><username>%EMAILLOCALPART%</username><authentication>password-cleartext</authentication></incomingServer>
        <outgoingServer type="smtp"><hostname>smtp.example.com</hostname><port>587</port><socketType>STARTTLS</socketType><username>%EMAILADDRESS%</username><authentication>password-cleartext</authentication></outgoingServer>
        </emailProvider></clientConfig>"#;
    #[tokio::test]
    async fn domain_table_never_uses_network() {
        for domain in [
            "gmail.com",
            "GOOGLEMAIL.COM",
            "outlook.com",
            "hotmail.co.uk",
            "live.se",
            "msn.com",
            "icloud.com",
            "me.com",
            "mac.com",
            "fastmail.com",
            "fastmail.fm",
        ] {
            let email = format!("alice@{domain}");
            let result = discover_with(
                &email,
                |_| async { panic!("known domain did DNS") },
                |_| async { panic!("known domain did HTTP") },
            )
            .await;
            match domain {
                "gmail.com" | "GOOGLEMAIL.COM" => {
                    assert_eq!(result, Discovery::OAuth(Provider::Google))
                }
                "outlook.com" | "hotmail.co.uk" | "live.se" | "msn.com" => {
                    assert_eq!(result, Discovery::OAuth(Provider::Microsoft))
                }
                _ => {
                    let Discovery::Password { preset, help_url } = result else {
                        panic!("expected password")
                    };
                    assert_eq!(preset.imap.username, email);
                    assert_eq!(
                        help_url.unwrap(),
                        if domain.starts_with("fastmail.") {
                            "https://app.fastmail.com/settings/security/apppasswords"
                        } else {
                            "https://account.apple.com/account/manage"
                        }
                    );
                }
            }
        }
    }
    #[tokio::test]
    async fn fake_mx_and_fallback() {
        for host in [
            "ASPMX.L.GOOGLE.COM.",
            "mx.googlemail.com",
            "tenant.mail.protection.outlook.com",
            "mx01.icloud.com",
            "in1-smtp.messagingengine.com",
        ] {
            let result = discover_with(
                "alice@example.com",
                |_| async { Some(vec![host.into()]) },
                |_| async { panic!("matched MX did HTTP") },
            )
            .await;
            assert_eq!(Some(result), mx_provider(host, "alice@example.com"));
        }
        for host in [
            "evilgoogle.com",
            "google.com.evil.test",
            "noticloud.com",
            "outlook.com",
            "",
        ] {
            assert!(mx_provider(host, "a@example.com").is_none());
        }
        let result = discover_with(
            "alice@example.com",
            |_| async { None },
            |_| async { Some(XML.into()) },
        )
        .await;
        let Discovery::Password { preset, help_url } = result else {
            panic!("expected ISPDB")
        };
        assert_eq!(preset.imap.username, "alice");
        assert_eq!(preset.imap.host, "imap.example.com");
        assert_eq!(preset.smtp.username, "alice@example.com");
        assert!(help_url.is_none());
        assert_eq!(
            discover_with(
                "alice@example.com",
                |_| async { Some(vec!["unknown.test".into()]) },
                |_| async { None }
            )
            .await,
            Discovery::Manual
        );
    }
    #[tokio::test]
    async fn timeouts_fall_through() {
        assert!(matches!(
            discover_with(
                "a@example.com",
                |_| std::future::pending(),
                |_| async { Some(XML.into()) }
            )
            .await,
            Discovery::Password { .. }
        ));
        assert_eq!(
            discover_with(
                "a@example.com",
                |_| async { None },
                |_| std::future::pending()
            )
            .await,
            Discovery::Manual
        );
    }
    #[test]
    fn malformed_or_unsafe_ispdb_is_not_used() {
        for xml in [
            "junk".into(),
            XML.replace("SSL", "plain"),
            XML.replace("993", "0"),
            XML.replace("password-cleartext", "OAuth2"),
            XML.replace("%EMAILLOCALPART%", "%UNKNOWN%"),
            XML.replace("imap.%EMAILDOMAIN%", "bad host"),
        ] {
            assert!(parse_autoconfig(&xml, "a@example.com").is_none());
        }
    }
    #[tokio::test]
    async fn malformed_addresses_do_not_use_network() {
        for email in [
            "",
            "a",
            "a@",
            "a@../../evil",
            "a@example.com\n",
            "a@[127.0.0.1]",
        ] {
            assert_eq!(
                discover_with(
                    email,
                    |_| async { panic!("invalid DNS") },
                    |_| async { panic!("invalid HTTP") }
                )
                .await,
                Discovery::Manual
            );
        }
    }
}
