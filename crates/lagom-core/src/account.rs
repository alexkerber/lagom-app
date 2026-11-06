use crate::{Error, Result};
use std::net::IpAddr;
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Security {
    Tls,
    StartTls,
    Plain,
}
impl Security {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Tls => "tls",
            Self::StartTls => "starttls",
            Self::Plain => "plain",
        }
    }
    pub(crate) fn parse(s: &str) -> Result<Self> {
        match s {
            "tls" => Ok(Self::Tls),
            "starttls" => Ok(Self::StartTls),
            "plain" => Ok(Self::Plain),
            _ => Err(Error::Invalid("unknown transport security")),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub security: Security,
    pub username: String,
}
impl ServerConfig {
    pub fn validate(&self) -> Result<()> {
        if self.host.is_empty()
            || self
                .host
                .chars()
                .any(|c| c.is_whitespace() || c.is_control())
            || self.port == 0
            || self.username.is_empty()
            || self.username.chars().any(char::is_control)
        {
            return Err(Error::Invalid("invalid server configuration"));
        }
        if self.security == Security::Plain
            && !self.host.eq_ignore_ascii_case("localhost")
            && !self.host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
        {
            return Err(Error::Invalid(
                "unencrypted connections require localhost or a loopback IP",
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub id: String,
    pub display_name: String,
    pub email: String,
    pub imap: ServerConfig,
    pub smtp: ServerConfig,
}
impl Account {
    pub fn new(email: String, imap: ServerConfig, smtp: ServerConfig) -> Result<Self> {
        let account = Self {
            id: uuid::Uuid::new_v4().to_string(),
            display_name: email.clone(),
            email,
            imap,
            smtp,
        };
        account.validate()?;
        Ok(account)
    }
    pub fn validate(&self) -> Result<()> {
        if self.id.is_empty()
            || self
                .email
                .chars()
                .any(|c| c.is_whitespace() || c.is_control())
            || self.email.parse::<lettre::Address>().is_err()
            || self.display_name.chars().any(char::is_control)
        {
            return Err(Error::Invalid("invalid account"));
        }
        self.imap.validate()?;
        self.smtp.validate()
    }
    fn secret_entry(&self) -> Result<keyring::Entry> {
        keyring::Entry::new("lagom", &self.id).map_err(|_| Error::Secret)
    }
    pub fn set_password(&self, password: &str) -> Result<()> {
        self.secret_entry()?
            .set_password(password)
            .map_err(|_| Error::Secret)
    }
    pub fn password(&self) -> Result<Zeroizing<String>> {
        self.secret_entry()?
            .get_password()
            .map(Zeroizing::new)
            .map_err(|_| Error::Secret)
    }
    pub fn delete_password(&self) -> Result<()> {
        self.secret_entry()?
            .delete_credential()
            .map_err(|_| Error::Secret)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plain_is_loopback_only() {
        for host in ["localhost", "LOCALHOST", "127.0.0.1", "::1"] {
            assert!(ServerConfig {
                host: host.into(),
                port: 3143,
                security: Security::Plain,
                username: "test".into()
            }
            .validate()
            .is_ok());
        }
        for host in ["mail.example.com", "localhost.evil", "192.168.1.1", ""] {
            assert!(ServerConfig {
                host: host.into(),
                port: 3143,
                security: Security::Plain,
                username: "test".into()
            }
            .validate()
            .is_err());
        }
    }
    #[test]
    fn invalid_account_addresses_are_rejected() {
        let (imap, smtp) = crate::presets::servers("a@gmail.com").unwrap();
        for email in [
            "@gmail.com",
            "a@",
            "bad address@example.com",
            "a@example.com\r\n",
        ] {
            assert!(Account::new(email.into(), imap.clone(), smtp.clone()).is_err());
        }
    }
    #[test]
    fn keychain_contract_with_mock() {
        keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
        let entry = keyring::Entry::new("lagom", "unit-test").unwrap();
        entry.set_password("test secret").unwrap();
        assert_eq!(entry.get_password().unwrap(), "test secret");
        entry.delete_credential().unwrap();
        assert!(entry.get_password().is_err());
    }
}
