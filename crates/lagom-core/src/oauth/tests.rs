use super::*;
use keyring::credential::{CredentialApi, CredentialBuilderApi};
use tokio::io::{AsyncBufReadExt, BufReader};

#[test]
fn pkce_and_callback_validation() {
    assert_eq!(
        challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
    assert_eq!(
        &*parse_callback("/?code=a%2Bb&state=expected", "expected").unwrap(),
        "a+b"
    );
    assert!(matches!(
        parse_callback("/?error=access_denied&state=s", "s"),
        Err(Error::Cancelled)
    ));
    for target in [
        "/?code=c&state=wrong",
        "/?error=access_denied&state=wrong",
        "/?code=c",
        "/?state=s",
        "/?code=c&state=s&state=s",
        "/other?code=c&state=s",
    ] {
        assert!(parse_callback(target, "s").is_err());
    }
}
fn jwt(payload: &str) -> String {
    format!("header.{}.signature", URL_SAFE_NO_PAD.encode(payload))
}
#[test]
fn jwt_identity() {
    assert_eq!(
        claims(&jwt(r#"{"email":"a@gmail.com","name":"Alice"}"#)).unwrap(),
        ("a@gmail.com".into(), "Alice".into())
    );
    assert_eq!(
        claims(&jwt(r#"{"preferred_username":"a@outlook.com"}"#))
            .unwrap()
            .0,
        "a@outlook.com"
    );
    for token in [
        "invalid".into(),
        "a.%.c".into(),
        jwt("{}"),
        jwt(r#"{"email":"a\n@b.com"}"#),
    ] {
        assert!(claims(&token).is_err());
    }
}

// A persistent in-memory keychain for the end-to-end flow; OS credentials are never touched.
#[derive(Debug, Clone, Default)]
struct Memory(Arc<Mutex<HashMap<String, String>>>);
#[derive(Debug)]
struct Credential {
    memory: Memory,
    key: String,
}
impl CredentialApi for Credential {
    fn set_secret(&self, value: &[u8]) -> keyring::Result<()> {
        self.set_password(std::str::from_utf8(value).unwrap())
    }
    fn get_secret(&self) -> keyring::Result<Vec<u8>> {
        self.get_password().map(String::into_bytes)
    }
    fn set_password(&self, value: &str) -> keyring::Result<()> {
        self.memory
            .0
            .lock()
            .unwrap()
            .insert(self.key.clone(), value.into());
        Ok(())
    }
    fn get_password(&self) -> keyring::Result<String> {
        self.memory
            .0
            .lock()
            .unwrap()
            .get(&self.key)
            .cloned()
            .ok_or(keyring::Error::NoEntry)
    }
    fn delete_credential(&self) -> keyring::Result<()> {
        self.memory.0.lock().unwrap().remove(&self.key);
        Ok(())
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn debug_fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("test credential")
    }
}
impl CredentialBuilderApi for Memory {
    fn build(
        &self,
        _: Option<&str>,
        service: &str,
        user: &str,
    ) -> keyring::Result<Box<keyring::credential::Credential>> {
        Ok(Box::new(Credential {
            memory: self.clone(),
            key: format!("{service}/{user}"),
        }))
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
fn memory() -> &'static Memory {
    static MEMORY: OnceLock<Memory> = OnceLock::new();
    MEMORY.get_or_init(|| {
        let memory = Memory::default();
        keyring::set_default_credential_builder(Box::new(memory.clone()));
        memory
    })
}

#[tokio::test]
async fn reauthentication_preserves_account_and_rejects_another_identity_before_writing() {
    let memory = memory();
    for provider in [Provider::Google, Provider::Microsoft] {
        let (imap, smtp) = provider.servers("fixture@example.com");
        let mut account = Account::new("fixture@example.com".into(), imap, smtp).unwrap();
        account.auth = Auth::OAuth(provider);
        account.display_name = "Custom label".into();
        account.set_password("old-refresh").unwrap();
        let token = |email: &str| Token {
            scope: None,
            access_token: "new-access".into(), expires_in: 3600, token_type: "Bearer".into(),
            refresh_token: Some("reauth-fixture-refresh".into()),
            id_token: Some(jwt(&serde_json::json!({"email": email, "name": "Provider label"}).to_string())),
        };
        let mut wrong = token("other@example.com");
        wrong.refresh_token = Some("must-not-save".into());
        assert!(matches!(complete_sign_in(provider, Some(&account), &mut wrong).await,
            Err(Error::Invalid("Sign in with the same email address as this account"))));
        assert_eq!(&*account.password().unwrap(), "old-refresh");
        assert!(!memory.0.lock().unwrap().values().any(|value| value == "must-not-save"));
        assert!(cache(&account.id).lock().await.is_none());
        let mut missing = token(&account.email);
        missing.refresh_token = None;
        assert!(matches!(complete_sign_in(provider, Some(&account), &mut missing).await, Err(Error::ReauthRequired)));
        assert_eq!(&*account.password().unwrap(), "old-refresh");
        let other_provider = if provider == Provider::Google { Provider::Microsoft } else { Provider::Google };
        assert!(complete_sign_in(other_provider, Some(&account), &mut token(&account.email)).await.is_err());
        let signed = complete_sign_in(provider, Some(&account), &mut token("FIXTURE@example.com")).await.unwrap();
        assert_eq!(signed.account, account);
        assert_eq!(&*account.password().unwrap(), "reauth-fixture-refresh");
        assert_eq!(cache(&account.id).lock().await.as_ref().unwrap().token.as_str(), "new-access");
        assert_eq!(memory.0.lock().unwrap().values().filter(|value| *value == "reauth-fixture-refresh").count(), 1);
        assert!(complete_sign_in(provider, Some(&account), &mut wrong).await.is_err());
        assert_eq!(&*account.password().unwrap(), "reauth-fixture-refresh");
        assert_eq!(cache(&account.id).lock().await.as_ref().unwrap().token.as_str(), "new-access");
        account.delete_password().unwrap();
    }
}

async fn request(
    socket: tokio::net::TcpStream,
) -> (BufReader<tokio::net::TcpStream>, String, String) {
    let mut reader = BufReader::new(socket);
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    let first = line.clone();
    let mut len = 0;
    loop {
        line.clear();
        reader.read_line(&mut line).await.unwrap();
        if line == "\r\n" {
            break;
        }
        if let Some(n) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            len = n.trim().parse().unwrap();
        }
    }
    let mut body = vec![0; len];
    reader.read_exact(&mut body).await.unwrap();
    (reader, first, String::from_utf8(body).unwrap())
}
async fn respond(stream: &mut (impl AsyncWriteExt + Unpin), status: &str, body: &str) {
    stream.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
}
#[tokio::test]
async fn browser_loopback_exchange_refresh_rotation_and_revocation() {
    memory();
    for provider in [Provider::Google, Provider::Microsoft] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let config = || Config {
            id: "test-id".into(),
            secret: Zeroizing::new(
                if provider == Provider::Google {
                    "test-secret"
                } else {
                    ""
                }
                .into(),
            ),
            auth: format!("{endpoint}/authorize"),
            token: format!("{endpoint}/token"),
            scope: provider.config().scope,
        };
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let (mut socket, first, _) = request(socket).await;
            let url = Url::parse(&format!(
                "http://localhost{}",
                first.split_whitespace().nth(1).unwrap()
            ))
            .unwrap();
            let params: HashMap<_, _> = url.query_pairs().into_owned().collect();
            assert_eq!(params["code_challenge_method"], "S256");
            assert_eq!(params["client_id"], "test-id");
            let redirect = Url::parse(&params["redirect_uri"]).unwrap();
            assert_eq!(
                redirect.host_str().unwrap(),
                if provider == Provider::Google {
                    "127.0.0.1"
                } else {
                    "localhost"
                }
            );
            respond(&mut socket, "200 OK", "{}").await;
            let mut callback =
                tokio::net::TcpStream::connect(("127.0.0.1", redirect.port().unwrap()))
                    .await
                    .unwrap();
            callback
                .write_all(
                    format!(
                        "GET /?code=test-code&state={} HTTP/1.1\r\nHost: localhost\r\n\r\n",
                        params["state"]
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            let mut response = String::new();
            callback.read_to_string(&mut response).await.unwrap();
            assert!(response.contains("You can close this tab and return to lagom."));
            for n in 0..3 {
                let (socket, _) = listener.accept().await.unwrap();
                let (mut socket, _, form) = request(socket).await;
                let form: HashMap<_, _> = url::form_urlencoded::parse(form.as_bytes())
                    .into_owned()
                    .collect();
                assert_eq!(form["client_id"], "test-id");
                assert_eq!(
                    form.contains_key("client_secret"),
                    provider == Provider::Google
                );
                if n == 0 {
                    assert_eq!(form["grant_type"], "authorization_code");
                    assert_eq!(challenge(&form["code_verifier"]), params["code_challenge"]);
                    assert_eq!(form["redirect_uri"], params["redirect_uri"]);
                    assert_eq!(form["code"], "test-code");
                } else {
                    assert_eq!(form["grant_type"], "refresh_token");
                    assert_eq!(form.contains_key("scope"), provider == Provider::Microsoft);
                    assert_eq!(
                        form["refresh_token"],
                        if n == 1 { "refresh-1" } else { "refresh-2" }
                    );
                }
                if n == 2 {
                    respond(
                        &mut socket,
                        "400 Bad Request",
                        r#"{"error":"invalid_grant","error_description":"secret-must-not-leak"}"#,
                    )
                    .await;
                } else {
                    let body = serde_json::json!({"access_token": format!("access-{n}"), "token_type":"Bearer", "expires_in":3600, "refresh_token":format!("refresh-{}", n+1), "id_token":jwt(r#"{"email":"alice@example.com","name":"Alice"}"#)}).to_string();
                    respond(&mut socket, "200 OK", &body).await;
                }
            }
        });
        let signed = sign_in_with(provider, config(), None, |url| {
            let url = url.to_owned();
            tokio::spawn(async move {
                http(Duration::from_secs(3))
                    .unwrap()
                    .get(url)
                    .send()
                    .await
                    .unwrap();
            });
        })
        .await
        .unwrap();
        assert_eq!(signed.email, "alice@example.com");
        assert_eq!(signed.account.auth, Auth::OAuth(provider));
        assert_eq!(&*signed.account.password().unwrap(), "refresh-1");
        assert_eq!(
            &*access_token_with(&signed.account, None, &config())
                .await
                .unwrap(),
            "access-0"
        );
        assert_eq!(
            &*access_token_with(&signed.account, Some("access-0"), &config())
                .await
                .unwrap(),
            "access-1"
        );
        assert_eq!(&*signed.account.password().unwrap(), "refresh-2");
        // A concurrent rejection of the old token reuses the refresh already completed.
        assert_eq!(
            &*access_token_with(&signed.account, Some("access-0"), &config())
                .await
                .unwrap(),
            "access-1"
        );
        cache(&signed.account.id)
            .lock()
            .await
            .as_mut()
            .unwrap()
            .expires = Instant::now();
        let error = access_token_with(&signed.account, None, &config())
            .await
            .unwrap_err();
        assert!(matches!(error, Error::ReauthRequired));
        assert!(!format!("{error:?}").contains("secret-must-not-leak"));
        signed.account.delete_password().unwrap();
        forget(&signed.account.id);
        server.await.unwrap();
    }
}
/// Sign-in on a local fake provider: one stray wrong-state request on 127.0.0.1, then the real callback on `host`.
async fn sign_in_after_a_stray_request(provider: Provider, host: &'static str, email: &'static str) {
    memory();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let config = Config { id: "test-id".into(), secret: Zeroizing::new("test-secret".into()), auth: format!("{endpoint}/authorize"), token: format!("{endpoint}/token"), scope: provider.config().scope };
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let (mut socket, first, _) = request(socket).await;
        let url = Url::parse(&format!("http://localhost{}", first.split_whitespace().nth(1).unwrap())).unwrap();
        let params: HashMap<_, _> = url.query_pairs().into_owned().collect();
        respond(&mut socket, "200 OK", "{}").await;
        let port = Url::parse(&params["redirect_uri"]).unwrap().port().unwrap();
        for (to, state, status) in [("127.0.0.1", "wrong", "400 Bad Request"), (host, params["state"].as_str(), "200 OK")] {
            let mut callback = tokio::net::TcpStream::connect((to, port)).await.unwrap();
            callback.write_all(format!("GET /?code=test-code&state={state} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes()).await.unwrap();
            let mut response = String::new();
            callback.read_to_string(&mut response).await.unwrap();
            assert!(response.starts_with(&format!("HTTP/1.1 {status}")), "{response}");
        }
        let (socket, _) = listener.accept().await.unwrap();
        let (mut socket, _, form) = request(socket).await;
        assert!(form.contains("code=test-code"), "{form}");
        let body = serde_json::json!({"access_token": "access", "token_type": "Bearer", "expires_in": 3600, "refresh_token": "refresh", "id_token": jwt(&format!(r#"{{"email":"{email}"}}"#))}).to_string();
        respond(&mut socket, "200 OK", &body).await;
    });
    let signed = sign_in_with(provider, config, None, |url| {
        let url = url.to_owned();
        tokio::spawn(async move { http(Duration::from_secs(3)).unwrap().get(url).send().await.unwrap(); });
    }).await.unwrap();
    assert_eq!(signed.email, email);
    signed.account.delete_password().unwrap();
    forget(&signed.account.id);
    server.await.unwrap();
}
#[tokio::test]
async fn a_wrong_state_request_does_not_end_sign_in() {
    sign_in_after_a_stray_request(Provider::Google, "127.0.0.1", "stray-google@example.com").await;
}
#[tokio::test]
async fn microsoft_localhost_callback_is_accepted_on_ipv6_loopback() {
    // Without IPv6 loopback on this machine, sign-in runs on IPv4 alone.
    let host = if std::net::TcpListener::bind("[::1]:0").is_ok() { "::1" } else { "127.0.0.1" };
    sign_in_after_a_stray_request(Provider::Microsoft, host, "stray-microsoft@example.com").await;
}
#[tokio::test]
async fn dropping_sign_in_closes_the_listener() {
    let mut redirect = None;
    let mut config = Provider::Google.config();
    config.id = "test".into();
    let mut flow = Box::pin(sign_in_with(Provider::Google, config, None, |url| {
        redirect = Url::parse(url)
            .unwrap()
            .query_pairs()
            .find(|(k, _)| k == "redirect_uri")
            .map(|(_, v)| v.into_owned());
    }));
    assert!(tokio::time::timeout(Duration::from_millis(30), &mut flow)
        .await
        .is_err());
    drop(flow);
    let redirect = Url::parse(&redirect.unwrap()).unwrap();
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", redirect.port().unwrap()))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn calendar_granular_consent_preserves_mail_and_saves_only_granted_scopes() {
    memory();
    let (imap, smtp) = Provider::Google.servers("fixture@example.com");
    let mut account = Account::new("fixture@example.com".into(), imap, smtp).unwrap();
    account.auth = Auth::OAuth(Provider::Google);
    account.set_password("original-fixture").unwrap();
    let mut token = Token {
        access_token: "fixture-access".into(), expires_in: 3600, token_type: "Bearer".into(),
        refresh_token: Some("replacement-fixture".into()), id_token: Some(jwt(r#"{"email":"fixture@example.com"}"#)),
        scope: Some(GOOGLE_CALENDAR_SCOPES.into()),
    };
    assert!(complete_sign_in(Provider::Google, Some(&account), &mut token).await.is_err());
    assert_eq!(&*account.password().unwrap(), "original-fixture");
    assert!(cache(&account.id).lock().await.is_none());
    token.scope = Some("https://mail.google.com/ https://www.googleapis.com/auth/calendar.events".into());
    let signed = complete_sign_in(Provider::Google, Some(&account), &mut token).await.unwrap();
    assert_eq!(signed.scopes, token.scope.as_deref().unwrap());
    assert!(!calendar_allowed(&signed.scopes));
    assert_eq!(&*account.password().unwrap(), "replacement-fixture");
    assert!(calendar_allowed(GOOGLE_CALENDAR_SCOPES));
    assert!(!calendar_allowed("https://www.googleapis.com/auth/calendar.events.extra https://www.googleapis.com/auth/calendar.calendarlist.readonly"));
    account.delete_password().unwrap();
}

#[tokio::test]
async fn calendar_consent_requests_full_scopes_and_existing_identity_hint() {
    let (imap, smtp) = Provider::Google.servers("fixture@example.com");
    let mut account = Account::new("fixture@example.com".into(), imap, smtp).unwrap();
    account.auth = Auth::OAuth(Provider::Google);
    let mut config = Provider::Google.config();
    config.id = "fixture-id".into();
    config.scope.push(' ');
    config.scope.push_str(GOOGLE_CALENDAR_SCOPES);
    let mut observed = None;
    let mut flow = Box::pin(sign_in_with(Provider::Google, config, Some(&account), |url| { observed = Some(url.to_string()); }));
    assert!(tokio::time::timeout(Duration::from_millis(30), &mut flow).await.is_err());
    drop(flow);
    let url = Url::parse(&observed.unwrap()).unwrap();
    let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(query["login_hint"], account.email);
    assert_eq!(query["prompt"], "consent");
    assert!(!query.contains_key("include_granted_scopes"));
    assert!(has_scope(&query["scope"], "https://mail.google.com/"));
    for scope in GOOGLE_CALENDAR_SCOPES.split_whitespace() { assert!(has_scope(&query["scope"], scope)); }
}
