//! Browser-based installed-app OAuth. The existing GPUI reqwest fork shares its
//! rustls/Tokio stack and supports cancellation without blocking HTTP threads.
use crate::{
    account::{Account, Auth, Security, ServerConfig},
    Error, Result,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use url::Url;
use zeroize::{Zeroize, Zeroizing};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Google,
    Microsoft,
}

impl Provider {
    fn config(self) -> Config {
        let (id, secret, auth, token, scope) = match self {
            Self::Google => (
                env("LAGOM_GOOGLE_CLIENT_ID", option_env!("LAGOM_GOOGLE_CLIENT_ID")),
                env("LAGOM_GOOGLE_CLIENT_SECRET", option_env!("LAGOM_GOOGLE_CLIENT_SECRET")),
                "https://accounts.google.com/o/oauth2/v2/auth",
                "https://oauth2.googleapis.com/token",
                "https://mail.google.com/ openid email profile",
            ),
            Self::Microsoft => (
                env("LAGOM_MICROSOFT_CLIENT_ID", option_env!("LAGOM_MICROSOFT_CLIENT_ID")),
                String::new(),
                "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
                "https://login.microsoftonline.com/common/oauth2/v2.0/token",
                "https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send offline_access openid email profile",
            ),
        };
        Config {
            id,
            secret: Zeroizing::new(secret),
            auth: auth.into(),
            token: token.into(),
            scope,
        }
    }
    pub fn servers(self, email: &str) -> (ServerConfig, ServerConfig) {
        let (imap, smtp) = match self {
            Self::Google => ("imap.gmail.com", "smtp.gmail.com"),
            Self::Microsoft => ("outlook.office365.com", "smtp.office365.com"),
        };
        (
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
        )
    }
}
fn env(name: &str, built: Option<&str>) -> String {
    std::env::var(name).unwrap_or_else(|_| built.unwrap_or_default().into())
}
pub fn available(provider: Provider) -> bool {
    !provider.config().id.trim().is_empty()
}

struct Config {
    id: String,
    secret: Zeroizing<String>,
    auth: String,
    token: String,
    scope: &'static str,
}
#[derive(Debug)]
pub struct SignedIn {
    pub email: String,
    pub display_name: String,
    pub account: Account,
}

fn random() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Cancels when dropped; the listener and HTTP request are owned by this future.
/// The callback runs on the caller's executor; desktop callers forward the URL to the UI thread.
pub async fn sign_in(provider: Provider, open_url: impl FnOnce(&str)) -> Result<SignedIn> {
    sign_in_with(provider, provider.config(), open_url).await
}
async fn sign_in_with(
    provider: Provider,
    config: Config,
    open_url: impl FnOnce(&str),
) -> Result<SignedIn> {
    if config.id.trim().is_empty() {
        return Err(Error::Invalid("sign-in provider is not configured"));
    }
    tokio::time::timeout(Duration::from_secs(300), async {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let host = match provider {
            Provider::Google => "127.0.0.1",
            Provider::Microsoft => "localhost",
        };
        let redirect = format!("http://{host}:{}/", listener.local_addr()?.port());
        let state = random();
        let verifier = Zeroizing::new(random());
        let mut url = Url::parse(&config.auth)
            .map_err(|_| Error::Invalid("invalid authorization endpoint"))?;
        url.query_pairs_mut().extend_pairs([
            ("client_id", config.id.as_str()),
            ("redirect_uri", &redirect),
            ("response_type", "code"),
            ("scope", config.scope),
            ("state", &state),
            ("code_challenge", &challenge(&verifier)),
            ("code_challenge_method", "S256"),
        ]);
        if provider == Provider::Google {
            url.query_pairs_mut()
                .extend_pairs([("access_type", "offline"), ("prompt", "consent")]);
        }
        open_url(url.as_str());
        let code = callback(&listener, &state).await?;
        let mut token = exchange(
            &config,
            &[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", &redirect),
                ("code_verifier", &verifier),
            ],
        )
        .await?;
        let (email, display_name) = claims(
            token
                .id_token
                .as_deref()
                .ok_or(Error::Invalid("ID token missing"))?,
        )?;
        let (imap, smtp) = provider.servers(&email);
        let mut account = Account::new(email.clone(), imap, smtp)?;
        account.auth = Auth::OAuth(provider);
        account.display_name = display_name.clone();
        account.validate()?;
        let refresh = token
            .refresh_token
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or(Error::ReauthRequired)?;
        let cache = cache(&account.id);
        let mut cached = cache.lock().await;
        // No cancellation point between persisting the credential and returning its account.
        account.set_password(refresh)?;
        *cached = Some(Cached::from_token(&mut token));
        Ok(SignedIn {
            email,
            display_name,
            account,
        })
    })
    .await
    .map_err(|_| Error::Network("sign-in timed out"))?
}

fn parse_callback(target: &str, state: &str) -> Result<Zeroizing<String>> {
    if !target.starts_with("/?") {
        return Err(Error::Invalid("invalid sign-in callback"));
    }
    let url = Url::parse(&format!("http://localhost{target}"))
        .map_err(|_| Error::Invalid("invalid sign-in callback"))?;
    let pairs: Vec<_> = url.query_pairs().collect();
    let one = |key: &str| {
        let mut values = pairs
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.as_ref());
        let value = values.next();
        if values.next().is_some() {
            None
        } else {
            value
        }
    };
    if one("state") != Some(state) {
        return Err(Error::Invalid("sign-in state mismatch"));
    }
    if let Some(error) = one("error") {
        return Err(if error == "access_denied" {
            Error::Cancelled
        } else {
            Error::Network("authorization failed")
        });
    }
    one("code")
        .filter(|s| !s.is_empty())
        .map(|s| Zeroizing::new(s.into()))
        .ok_or(Error::Invalid("authorization code missing"))
}
async fn callback(listener: &TcpListener, state: &str) -> Result<Zeroizing<String>> {
    loop {
        let (mut stream, _) = listener.accept().await?;
        let request = tokio::time::timeout(Duration::from_secs(3), async {
            let mut bytes = Zeroizing::new(Vec::new());
            while bytes.len() < 8192 && !bytes.ends_with(b"\r\n\r\n") {
                let byte = stream.read_u8().await?;
                bytes.push(byte);
            }
            Ok::<_, std::io::Error>(bytes)
        })
        .await;
        let Ok(Ok(request)) = request else { continue };
        let line = std::str::from_utf8(&request)
            .ok()
            .and_then(|r| r.lines().next())
            .unwrap_or("");
        let parts: Vec<_> = line.split_whitespace().collect();
        if parts.len() != 3 || parts[0] != "GET" || !parts[2].starts_with("HTTP/1.") {
            continue;
        }
        // Browsers may request a favicon before/after the redirect; it is not an OAuth response.
        if !parts[1].starts_with("/?") {
            continue;
        }
        let result = parse_callback(parts[1], state);
        let body =
            "<!doctype html><title>Lagom</title><p>You can close this tab and return to Lagom.</p>";
        let status = if result.is_ok() {
            "200 OK"
        } else {
            "400 Bad Request"
        };
        let reply = format!("HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nContent-Security-Policy: default-src 'none'\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}", body.len());
        let _ =
            tokio::time::timeout(Duration::from_secs(3), stream.write_all(reply.as_bytes())).await;
        return result;
    }
}

#[derive(Deserialize)]
struct Claims {
    email: Option<String>,
    preferred_username: Option<String>,
    name: Option<String>,
}
fn claims(jwt: &str) -> Result<(String, String)> {
    let parts: Vec<_> = jwt.split('.').collect();
    if parts.len() != 3 {
        return Err(Error::Invalid("invalid ID token"));
    }
    // No signature check: this ID token came directly from the token endpoint over verified TLS.
    let payload = Zeroizing::new(
        URL_SAFE_NO_PAD
            .decode(parts[1])
            .map_err(|_| Error::Invalid("invalid ID token"))?,
    );
    let claims: Claims =
        serde_json::from_slice(&payload).map_err(|_| Error::Invalid("invalid ID token claims"))?;
    let email = claims
        .email
        .or(claims.preferred_username)
        .ok_or(Error::Invalid("email missing from ID token"))?;
    if email.parse::<lettre::Address>().is_err()
        || email.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(Error::Invalid("invalid ID token email"));
    }
    let name = claims.name.unwrap_or_else(|| email.clone());
    if name.chars().any(char::is_control) {
        return Err(Error::Invalid("invalid ID token name"));
    }
    Ok((email, name))
}

pub(crate) fn http(timeout: Duration) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .use_rustls_tls()
        .timeout(timeout)
        // Never forward credentials to a redirect target.
        .redirect_policy(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| Error::Network("HTTP client failed"))
}
pub(crate) async fn body(mut response: reqwest::Response) -> Result<Zeroizing<Vec<u8>>> {
    let mut bytes = Zeroizing::new(Vec::new());
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| Error::Network("HTTP response failed"))?
    {
        if bytes.len() + chunk.len() > 256 * 1024 {
            return Err(Error::Network("HTTP response too large"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
#[derive(Deserialize)]
struct Token {
    access_token: String,
    expires_in: u64,
    token_type: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
}
impl Drop for Token {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
        self.id_token.zeroize();
    }
}
async fn exchange(config: &Config, fields: &[(&str, &str)]) -> Result<Token> {
    let form = {
        let mut form = url::form_urlencoded::Serializer::new(String::new());
        form.extend_pairs(fields.iter().copied()).append_pair("client_id", &config.id);
        if !config.secret.is_empty() { form.append_pair("client_secret", &config.secret); }
        Zeroizing::new(form.finish())
    };
    let response = http(Duration::from_secs(30))?
        .post(&config.token)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form.to_string())
        .send()
        .await
        .map_err(|_| Error::Network("token endpoint unavailable"))?;
    let status = response.status();
    let bytes = body(response).await?;
    if !status.is_success() {
        #[derive(Deserialize)]
        struct Failure {
            error: String,
        }
        return Err(match serde_json::from_slice::<Failure>(&bytes) {
            Ok(f) if f.error == "invalid_grant" => Error::ReauthRequired,
            _ => Error::Network("token request rejected"),
        });
    }
    let token: Token =
        serde_json::from_slice(&bytes).map_err(|_| Error::Network("invalid token response"))?;
    if token.access_token.is_empty()
        || token.access_token.chars().any(char::is_control)
        || !token.token_type.eq_ignore_ascii_case("Bearer")
        || token.expires_in == 0
    {
        return Err(Error::Network("invalid access token"));
    }
    Ok(token)
}
struct Cached {
    token: Zeroizing<String>,
    expires: Instant,
}
impl Cached {
    fn from_token(token: &mut Token) -> Self {
        Self {
            token: Zeroizing::new(std::mem::take(&mut token.access_token)),
            expires: Instant::now()
                + Duration::from_secs(token.expires_in.min(86400).saturating_sub(30)),
        }
    }
}
type TokenCache = Arc<tokio::sync::Mutex<Option<Cached>>>;
fn caches() -> &'static Mutex<HashMap<String, TokenCache>> {
    static CACHE: OnceLock<Mutex<HashMap<String, TokenCache>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}
fn cache(id: &str) -> TokenCache {
    caches()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(id.into())
        .or_default()
        .clone()
}
pub(crate) fn forget(id: &str) {
    caches()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(id);
}
/// `rejected` is the token refused by the server. Concurrent IMAP/SMTP retries share one refresh.
pub async fn access_token(account: &Account, rejected: Option<&str>) -> Result<Zeroizing<String>> {
    let Auth::OAuth(provider) = account.auth else {
        return Err(Error::Invalid("not an OAuth account"));
    };
    access_token_with(account, rejected, &provider.config()).await
}
async fn access_token_with(
    account: &Account,
    rejected: Option<&str>,
    config: &Config,
) -> Result<Zeroizing<String>> {
    if config.id.trim().is_empty() {
        return Err(Error::Invalid("sign-in provider is not configured"));
    }
    let cache = cache(&account.id);
    let mut cached = cache.lock().await;
    if let Some(c) = cached
        .as_ref()
        .filter(|c| c.expires > Instant::now() && rejected != Some(c.token.as_str()))
    {
        return Ok(c.token.clone());
    }
    *cached = None;
    let refresh = account.password()?;
    let mut token = exchange(
        config,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh),
            ("scope", config.scope),
        ],
    )
    .await?;
    if let Some(refresh) = token.refresh_token.as_deref().filter(|s| !s.is_empty()) {
        account.set_password(refresh)?;
    }
    let c = Cached::from_token(&mut token);
    let result = c.token.clone();
    *cached = Some(c);
    Ok(result)
}

#[cfg(test)]
mod tests;
