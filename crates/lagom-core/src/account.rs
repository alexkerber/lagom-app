use crate::{oauth::Provider, Error, Result};
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Auth {
    #[default]
    Password,
    OAuth(Provider),
}
impl Auth {
    /// How the account signs in, in the words Settings shows on every platform.
    pub fn kind(self) -> &'static str {
        match self {
            Self::Password => "Email and password",
            Self::OAuth(Provider::Google) => "Google account",
            Self::OAuth(Provider::Microsoft) => "Microsoft account",
        }
    }
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::OAuth(Provider::Google) => "google",
            Self::OAuth(Provider::Microsoft) => "microsoft",
        }
    }
    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "password" => Ok(Self::Password),
            "google" => Ok(Self::OAuth(Provider::Google)),
            "microsoft" => Ok(Self::OAuth(Provider::Microsoft)),
            _ => Err(Error::Invalid("unknown account authentication")),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub id: String,
    pub auth: Auth,
    pub credential_key: Option<String>,
    pub display_name: String,
    pub email: String,
    pub imap: ServerConfig,
    pub smtp: ServerConfig,
}
impl Account {
    pub fn new(email: String, imap: ServerConfig, smtp: ServerConfig) -> Result<Self> {
        let account = Self {
            id: uuid::Uuid::new_v4().to_string(),
            auth: Auth::Password,
            credential_key: None,
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
    pub fn google_hosted_password(&self) -> bool {
        self.auth == Auth::Password && (self.imap.host.eq_ignore_ascii_case("imap.gmail.com")
            || self.email.rsplit_once('@').is_some_and(|(_, domain)|
                domain.eq_ignore_ascii_case("gmail.com") || domain.eq_ignore_ascii_case("googlemail.com")))
    }
    pub(crate) fn secret_key(&self) -> &str { self.credential_key.as_deref().unwrap_or(&self.id) }
    fn secret_service(&self) -> String {
        let base = std::env::var("LAGOM_KEYCHAIN_SERVICE").unwrap_or_else(|_| "lagom".into());
        match &self.credential_key {
            Some(key) => format!("{base}.{key}"),
            None => base,
        }
    }
    fn secret_entry(&self) -> Result<keyring::Entry> {
        keyring::Entry::new(&self.secret_service(), &self.id).map_err(|_| Error::Secret)
    }
    pub fn set_password(&self, password: &str) -> Result<()> {
        let entry = self.secret_entry()?;
        // iOS: readable only while unlocked and never in backups or device migration. Reads stay with
        // keyring, which matches service and account only, so older items keep working.
        #[cfg(target_os = "ios")]
        {
            use security_framework::{access_control::{ProtectionMode, SecAccessControl}, passwords::{set_generic_password_options, PasswordOptions}};
            let service = self.secret_service();
            let mut options = PasswordOptions::new_generic_password(&service, &self.id);
            options.set_access_control(SecAccessControl::create_with_protection(Some(ProtectionMode::AccessibleWhenUnlockedThisDeviceOnly), 0).map_err(|_| Error::Secret)?);
            // An update keeps an older item's class, so the item is replaced.
            let _ = entry.delete_credential();
            set_generic_password_options(password.as_bytes(), options).map_err(|_| Error::Secret)
        }
        #[cfg(not(target_os = "ios"))]
        entry.set_password(password).map_err(|_| Error::Secret)
    }
    pub fn password(&self) -> Result<Zeroizing<String>> {
        self.secret_entry()?
            .get_password()
            .map(Zeroizing::new)
            .map_err(|_| Error::Secret)
    }
    /// Ok when the secret is gone, also when it was never there.
    pub fn delete_password(&self) -> Result<()> {
        match self.secret_entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {
                crate::oauth::forget(self.secret_key());
                Ok(())
            },
            Err(_) => Err(Error::Secret),
        }
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
    fn calendar_offers_sign_in_for_google_hosted_password_accounts() {
        let (imap, smtp) = crate::presets::servers("a@gmail.com").unwrap();
        let mut account = Account::new("a@work.example".into(), imap, smtp).unwrap();
        assert!(account.google_hosted_password());
        account.imap.host = "other.example".into();
        assert!(!account.google_hosted_password());
        for email in ["a@GMAIL.com", "a@GoogleMail.com"] {
            account.email = email.into();
            assert!(account.google_hosted_password());
        }
        account.auth = Auth::OAuth(Provider::Google);
        assert!(!account.google_hosted_password());
    }
    #[test]
    fn keychain_contract_with_mock() {
        let entry = keyring::Entry::new_with_credential(Box::new(keyring::mock::MockCredential::default()));
        entry.set_password("test secret").unwrap();
        assert_eq!(entry.get_password().unwrap(), "test secret");
        entry.delete_credential().unwrap();
        assert!(entry.get_password().is_err());
    }
}
