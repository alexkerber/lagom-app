//! Add an account: pick a provider, then email and app password, optional server settings.
//! "Connect" hands the account to the mail worker, which logs in to IMAP and SMTP before it
//! saves anything. No Figma frames for this screen existed on 2026-11-06; it follows the spec
//! and the Concept D language of the empty inbox and compose.

use gpui::{
    div, prelude::*, px, relative, App, Context, DismissEvent, Entity, EventEmitter, FocusHandle,
    Focusable, FontWeight, KeyDownEvent, SharedString, Subscription, Window,
};
use lagom_core::account::{Account, Security, ServerConfig};
use zeroize::Zeroizing;

use crate::main_window::{button, icon, icon_button, mono, pressable};
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme::{font, radius, size, Theme, DEFAULT_ACCENT};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Provider {
    Gmail,
    ICloud,
    Fastmail,
    Outlook,
    Other,
}

impl Provider {
    pub const ALL: [Provider; 5] = [Provider::Gmail, Provider::ICloud, Provider::Fastmail, Provider::Outlook, Provider::Other];

    pub fn name(self) -> &'static str {
        match self {
            Provider::Gmail => "Gmail",
            Provider::ICloud => "iCloud",
            Provider::Fastmail => "Fastmail",
            Provider::Outlook => "Outlook",
            Provider::Other => "Other IMAP",
        }
    }

    /// An address on the provider's own domain: the key into the core's server presets.
    fn preset(self) -> Option<&'static str> {
        match self {
            Provider::Gmail => Some("a@gmail.com"),
            Provider::ICloud => Some("a@icloud.com"),
            Provider::Fastmail => Some("a@fastmail.com"),
            Provider::Outlook => Some("a@outlook.com"),
            Provider::Other => None,
        }
    }

    /// The provider's own page on app passwords.
    pub fn help(self) -> Option<&'static str> {
        match self {
            Provider::Gmail => Some("https://support.google.com/accounts/answer/185833"),
            Provider::ICloud => Some("https://support.apple.com/en-us/102654"),
            Provider::Fastmail => Some("https://www.fastmail.help/hc/en-us/articles/360058752854-App-passwords"),
            Provider::Outlook => Some("https://support.microsoft.com/en-us/account-billing/manage-app-passwords-for-two-step-verification-d6dc8c6d-4bf7-4851-ad95-6d07799387e9"),
            Provider::Other => None,
        }
    }

    pub fn note(self) -> &'static str {
        match self {
            Provider::Outlook => "Outlook.com needs Microsoft sign-in (OAuth2), which Lagom does not have yet. An app password works only where Microsoft still allows IMAP with one.",
            Provider::Other => "Your provider's IMAP and SMTP servers. Use an app password if the provider offers one.",
            Provider::Gmail => "Gmail needs an app password, not your Google password. It needs 2-step verification.",
            Provider::ICloud | Provider::Fastmail => "Use an app password, not your normal password.",
        }
    }

    pub fn host_hint(self) -> &'static str {
        match self {
            Provider::Gmail => "imap.gmail.com",
            Provider::ICloud => "imap.mail.me.com",
            Provider::Fastmail => "imap.fastmail.com",
            Provider::Outlook => "outlook.office365.com",
            Provider::Other => "any IMAP server",
        }
    }
}

/// The server fields as typed.
#[derive(Clone, Debug, PartialEq)]
pub struct Servers {
    pub imap_host: String,
    pub imap_port: String,
    pub imap_security: Security,
    pub smtp_host: String,
    pub smtp_port: String,
    pub smtp_security: Security,
}

impl Servers {
    pub fn for_provider(p: Provider) -> Self {
        match p.preset().and_then(lagom_core::presets::servers) {
            Some((i, s)) => Self {
                imap_host: i.host,
                imap_port: i.port.to_string(),
                imap_security: i.security,
                smtp_host: s.host,
                smtp_port: s.port.to_string(),
                smtp_security: s.security,
            },
            None => Self {
                imap_host: String::new(),
                imap_port: "993".into(),
                imap_security: Security::Tls,
                smtp_host: String::new(),
                smtp_port: "587".into(),
                smtp_security: Security::StartTls,
            },
        }
    }
}

/// The account the form describes, or what to fix, in words for the user.
pub fn build(email: &str, password: &str, s: &Servers) -> Result<Account, String> {
    let email = email.trim();
    if !email.contains('@') || email.starts_with('@') || email.ends_with('@') {
        return Err("Enter your full email address.".into());
    }
    if password.is_empty() {
        return Err("Enter the app password.".into());
    }
    let server = |label: &str, host: &str, port: &str, security| -> Result<ServerConfig, String> {
        let host = host.trim();
        if host.is_empty() {
            return Err(format!("Enter the {label} server."));
        }
        let port = port.trim().parse::<u16>().ok().filter(|p| *p > 0).ok_or(format!("The {label} port must be a number from 1 to 65535."))?;
        let config = ServerConfig { host: host.into(), port, security, username: email.into() };
        config.validate().map_err(|e| match security {
            Security::Plain => format!("Unencrypted {label} works only with localhost."),
            _ => format!("{label}: {e}."),
        })?;
        Ok(config)
    };
    let imap = server("IMAP", &s.imap_host, &s.imap_port, s.imap_security)?;
    let smtp = server("SMTP", &s.smtp_host, &s.smtp_port, s.smtp_security)?;
    Account::new(email.into(), imap, smtp).map_err(|_| "That email address is not valid.".into())
}

fn security_label(s: Security) -> &'static str {
    match s {
        Security::Tls => "TLS",
        Security::StartTls => "STARTTLS",
        Security::Plain => "None",
    }
}

#[derive(Clone, PartialEq)]
enum State {
    Pick,
    Form,
    Connecting,
    Failed(String),
}

/// "Connect": the worker verifies and saves the account.
pub struct Connect(pub Box<Account>, pub Zeroizing<String>);

pub struct AddAccount {
    state: State,
    provider: Provider,
    /// Accounts exist, so Cancel can go back to the inbox.
    can_cancel: bool,
    advanced: bool,
    focus: FocusHandle,
    email: Entity<TextInput>,
    password: Entity<TextInput>,
    imap_host: Entity<TextInput>,
    imap_port: Entity<TextInput>,
    smtp_host: Entity<TextInput>,
    smtp_port: Entity<TextInput>,
    imap_security: Security,
    smtp_security: Security,
    _fields: Vec<Subscription>,
}

impl EventEmitter<Connect> for AddAccount {}
impl EventEmitter<DismissEvent> for AddAccount {}

impl Focusable for AddAccount {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl AddAccount {
    pub fn new(can_cancel: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let field = |placeholder: &'static str, masked: bool, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let input = TextInput::new(window, cx).bordered().placeholder(placeholder);
                if masked { input.masked() } else { input }
            })
        };
        let email = field("you@example.com", false, window, cx);
        let password = field("App password", true, window, cx);
        let imap_host = field("imap.example.com", false, window, cx);
        let imap_port = field("993", false, window, cx);
        let smtp_host = field("smtp.example.com", false, window, cx);
        let smtp_port = field("587", false, window, cx);
        let fields = [&email, &password, &imap_host, &imap_port, &smtp_host, &smtp_port]
            .map(|f| cx.subscribe_in(f, window, Self::on_field))
            .into();
        let focus = cx.focus_handle();
        window.focus(&focus);
        Self {
            state: State::Pick,
            provider: Provider::Gmail,
            can_cancel,
            advanced: false,
            focus,
            email,
            password,
            imap_host,
            imap_port,
            smtp_host,
            smtp_port,
            imap_security: Security::Tls,
            smtp_security: Security::StartTls,
            _fields: fields,
        }
    }

    pub fn pick(&mut self, p: Provider, window: &mut Window, cx: &mut Context<Self>) {
        self.provider = p;
        self.advanced = p == Provider::Other;
        let s = Servers::for_provider(p);
        for (field, text) in [(&self.imap_host, &s.imap_host), (&self.imap_port, &s.imap_port), (&self.smtp_host, &s.smtp_host), (&self.smtp_port, &s.smtp_port)] {
            field.update(cx, |f, cx| f.set_text(text, cx));
        }
        (self.imap_security, self.smtp_security) = (s.imap_security, s.smtp_security);
        self.state = State::Form;
        window.focus(&self.email.focus_handle(cx));
        cx.notify();
    }

    /// Test hook (`LAGOM_TEST_ACCOUNT`): fills the form as typing would.
    pub fn fill(&mut self, email: &str, password: &str, s: &Servers, window: &mut Window, cx: &mut Context<Self>) {
        self.pick(Provider::Other, window, cx);
        for (field, text) in [(&self.email, email), (&self.password, password), (&self.imap_host, &s.imap_host), (&self.imap_port, &s.imap_port), (&self.smtp_host, &s.smtp_host), (&self.smtp_port, &s.smtp_port)] {
            field.update(cx, |f, cx| f.set_text(text, cx));
        }
        (self.imap_security, self.smtp_security) = (s.imap_security, s.smtp_security);
    }

    fn servers(&self, cx: &App) -> Servers {
        let text = |f: &Entity<TextInput>| f.read(cx).text().to_string();
        Servers {
            imap_host: text(&self.imap_host),
            imap_port: text(&self.imap_port),
            imap_security: self.imap_security,
            smtp_host: text(&self.smtp_host),
            smtp_port: text(&self.smtp_port),
            smtp_security: self.smtp_security,
        }
    }

    pub fn connect(&mut self, cx: &mut Context<Self>) {
        if self.state == State::Connecting || self.state == State::Pick {
            return;
        }
        let password = Zeroizing::new(self.password.read(cx).text().to_string());
        match build(self.email.read(cx).text(), &password, &self.servers(cx)) {
            Ok(account) => {
                self.state = State::Connecting;
                cx.emit(Connect(Box::new(account), password));
            }
            Err(e) => self.state = State::Failed(e),
        }
        cx.notify();
    }

    /// The worker could not log in.
    pub fn failed(&mut self, error: String, cx: &mut Context<Self>) {
        self.state = State::Failed(error);
        cx.notify();
    }

    fn on_field(&mut self, field: &Entity<TextInput>, e: &TextInputEvent, window: &mut Window, cx: &mut Context<Self>) {
        match e {
            TextInputEvent::Enter if *field == self.password && !self.advanced => self.connect(cx),
            TextInputEvent::Enter if *field == self.smtp_port => self.connect(cx),
            TextInputEvent::Enter => window.focus_next(),
            TextInputEvent::Escape => self.back(window, cx),
            TextInputEvent::Changed => {}
        }
    }

    pub(crate) fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.state {
            State::Pick if self.can_cancel => cx.emit(DismissEvent),
            State::Pick | State::Connecting => {}
            _ => {
                self.state = State::Pick;
                window.focus(&self.focus);
                cx.notify();
            }
        }
    }

    /// 1–5 pick a provider; the submit shortcut connects; Esc goes back.
    fn on_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        match k.key.as_str() {
            "escape" => self.back(window, cx),
            "enter" if crate::shortcuts::submit().matches(k) => self.connect(cx),
            n if self.state == State::Pick && !k.modifiers.platform => {
                let Some(p) = n.parse::<usize>().ok().and_then(|i| Provider::ALL.get(i.wrapping_sub(1))) else { return };
                self.pick(*p, window, cx);
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    fn picker(&self, t: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        let mut list = div().w_full().flex().flex_col().gap(px(2.)).pt(px(20.)).border_t_1().border_color(t.line);
        for (i, p) in Provider::ALL.into_iter().enumerate() {
            list = list.child(
                pressable(div().id(("provider", i)), t, false)
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .h(px(48.))
                    .px(px(12.))
                    .rounded(px(radius::CONTROL))
                    .on_click(cx.listener(move |this, _, window, cx| this.pick(p, window, cx)))
                    .child(icon("icons/account-add.svg", t.ink2, 16.))
                    .child(div().flex_1().text_size(px(size::BODY)).font_weight(FontWeight::MEDIUM).child(p.name()))
                    .child(mono(p.host_hint(), t.ink2))
                    .child(mono((i + 1).to_string(), t.ink2).px(px(6.)).border_1().border_color(t.line).rounded(px(radius::KEY))),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(headline("Add your mail."))
            .child(div().text_size(px(size::TITLE)).text_color(t.ink2).line_height(relative(1.4)).child("Lagom keeps a copy on this computer and talks to your provider directly. Nothing goes through us."))
            .child(list)
            .child(mono(if self.can_cancel { "1–5 pick · esc back" } else { "1–5 pick" }, t.ink2))
    }

    fn form(&self, t: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        let p = self.provider;
        let label = |text: &'static str| mono(text, t.ink2).w(px(84.)).flex_shrink_0();
        let row = |name: &'static str, input: &Entity<TextInput>| div().flex().items_center().gap(px(12.)).child(label(name)).child(div().flex_1().min_w(px(0.)).child(input.clone()));
        let security = |current: Security, imap: bool, cx: &mut Context<Self>| {
            let mut seg = div().flex().p(px(2.)).rounded(px(radius::CONTROL)).bg(t.subtle).border_1().border_color(t.line).text_size(px(size::SMALL));
            for s in [Security::Tls, Security::StartTls, Security::Plain] {
                let on = s == current;
                seg = seg.child(
                    button(t, security_label(s), None).id(SharedString::from(format!("sec-{imap}-{}", security_label(s))))
                        .debug_selector(move || format!("sec-{imap}-{}", security_label(s)))
                        .h(px(26.))
                        .px(px(10.))
                        .border_0()
                        .bg(t.subtle)
                        .rounded(px(6.))
                        .when(on, |d| d.bg(t.raised).font_weight(FontWeight::SEMIBOLD))
                        .when(!on, |d| d.text_color(t.ink2))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if imap { this.imap_security = s } else { this.smtp_security = s }
                            cx.notify();
                        })),
                );
            }
            seg
        };
        let server = |name: &'static str, host: &Entity<TextInput>, port: &Entity<TextInput>, current: Security, imap: bool, cx: &mut Context<Self>| {
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(label(name))
                .child(div().flex_1().min_w(px(0.)).child(host.clone()))
                .child(div().w(px(72.)).child(port.clone()))
                .child(security(current, imap, cx))
        };
        let status = match &self.state {
            State::Connecting => Some(mono(format!("Connecting to {}…", self.imap_host.read(cx).text()), t.ink2)),
            State::Failed(e) => Some(div().text_size(px(size::SNIPPET)).text_color(t.danger).child(e.clone())),
            _ => None,
        };

        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(
                icon_button(t, "back", "icons/previous.svg", "Back to providers · Esc")
                    .on_click(cx.listener(|this, _, window, cx| this.back(window, cx))),
            )
            .child(headline(p.name()))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .pt(px(20.))
                    .border_t_1()
                    .border_color(t.line)
                    .child(row("Email", &self.email))
                    .child(row("Password", &self.password))
                    .child(
                        div()
                            .pl(px(96.))
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .text_size(px(size::SNIPPET))
                            .text_color(t.ink2)
                            .line_height(relative(1.4))
                            .child(p.note())
                            .when_some(p.help(), |d, url| {
                                d.child(
                                    div()
                                        .id("help")
                                        .flex()
                                        .text_color(t.accent)
                                        .cursor_pointer()
                                        .hover(|s| s.underline())
                                        .on_click(move |_, _, cx| cx.open_url(url))
                                        .gap(px(6.)).child(icon("icons/external.svg", t.ink2, 16.)).child("How to create one"),
                                )
                            }),
                    ),
            )
            .child(
                pressable(div().id("advanced"), t, false)
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(30.))
                    .px(px(8.))
                    .ml(px(-8.))
                    .rounded(px(radius::CONTROL))
                    .text_color(t.ink2)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.advanced ^= true;
                        cx.notify();
                    }))
                    .child(icon(if self.advanced { "icons/chevron.svg" } else { "icons/chevron-right.svg" }, t.ink2, 16.))
                    .child("Server settings"),
            )
            .when(self.advanced, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .child(server("IMAP", &self.imap_host, &self.imap_port, self.imap_security, true, cx))
                        .child(server("SMTP", &self.smtp_host, &self.smtp_port, self.smtp_security, false, cx)),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(16.))
                    .pt(px(8.))
                    .child(
                        button(t, if self.state == State::Connecting { "Connecting" } else { "Connect" }, Some(t.accent))
                            .on_click(cx.listener(|this, _, _, cx| this.connect(cx))),
                    )
                    .children(status),
            )
    }
}

fn headline(text: impl Into<SharedString>) -> gpui::Div {
    div().text_size(px(size::DISPLAY)).font_weight(FontWeight::BOLD).child(text.into())
}

impl Render for AddAccount {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::for_appearance(window.appearance(), DEFAULT_ACCENT);
        let content = match self.state {
            State::Pick => self.picker(&t, cx),
            _ => self.form(&t, cx),
        };
        div()
            .id("add-account")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .size_full()
            .overflow_y_scroll()
            .flex()
            .justify_center()
            .px(px(24.))
            .py(px(56.))
            .text_color(t.ink)
            .font_family(font::ui())
            .text_size(px(size::BASE))
            .child(div().w_full().max_w(px(560.)).child(content))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local() -> Servers {
        Servers {
            imap_host: "127.0.0.1".into(),
            imap_port: "3143".into(),
            imap_security: Security::Plain,
            smtp_host: "localhost".into(),
            smtp_port: "3025".into(),
            smtp_security: Security::Plain,
        }
    }

    #[gpui::test]
    fn account_controls_render_icons_and_submit_validates_without_a_worker(cx: &mut gpui::TestAppContext) {
        cx.update(crate::text_input::bind_keys);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut view = AddAccount::new(true, window, cx);
            view.pick(Provider::Other, window, cx);
            view
        });
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        for (label, path) in [("Connect", "connect"), ("sec-false-STARTTLS", "access"), ("sec-false-None", "close")] {
            let control = cx.debug_bounds(label).unwrap();
            let glyph = cx.debug_bounds(format!("icon-icons/{path}.svg").leak()).unwrap();
            assert_eq!(glyph.size, gpui::size(px(16.), px(16.)));
            assert!(control.contains(&glyph.center()), "{label}");
        }
        let plain = cx.debug_bounds("sec-false-None").unwrap().center();
        cx.simulate_click(plain, Default::default());
        assert_eq!(view.read_with(cx, |v, _| v.smtp_security), Security::Plain);
        cx.simulate_keystrokes(crate::shortcuts::submit().keystroke);
        assert!(view.read_with(cx, |v, _| matches!(&v.state, State::Failed(error) if error == "Enter your full email address.")));
    }

    #[test]
    fn presets_fill_the_servers() {
        let s = Servers::for_provider(Provider::Gmail);
        assert_eq!((s.imap_host.as_str(), s.imap_port.as_str(), s.smtp_host.as_str()), ("imap.gmail.com", "993", "smtp.gmail.com"));
        // A Fastmail account on a custom domain still gets Fastmail's servers.
        let a = build(" me@custom.se ", "pw", &Servers::for_provider(Provider::Fastmail)).unwrap();
        assert_eq!((a.email.as_str(), a.imap.host.as_str(), a.imap.username.as_str()), ("me@custom.se", "imap.fastmail.com", "me@custom.se"));
        assert!(Servers::for_provider(Provider::Other).imap_host.is_empty());
        assert!(Provider::ALL.iter().filter(|p| **p != Provider::Other).all(|p| p.help().is_some_and(|u| u.starts_with("https://"))));
    }

    #[test]
    fn form_validation_says_what_to_fix() {
        let gmail = Servers::for_provider(Provider::Gmail);
        assert_eq!(build("me", "pw", &gmail).unwrap_err(), "Enter your full email address.");
        assert_eq!(build("me@gmail.com", "", &gmail).unwrap_err(), "Enter the app password.");
        assert_eq!(build("bad address@x.se", "pw", &gmail).unwrap_err(), "That email address is not valid.");
        let other = Servers::for_provider(Provider::Other);
        assert_eq!(build("me@x.se", "pw", &other).unwrap_err(), "Enter the IMAP server.");
        let mut s = local();
        s.imap_port = "99999".into();
        assert_eq!(build("me@x.se", "pw", &s).unwrap_err(), "The IMAP port must be a number from 1 to 65535.");
        s = local();
        s.smtp_host = "smtp.x.se".into();
        assert_eq!(build("me@x.se", "pw", &s).unwrap_err(), "Unencrypted SMTP works only with localhost.");
        let a = build("test@lagom.test", "pw", &local()).unwrap();
        assert_eq!((a.imap.port, a.smtp.port, a.smtp.security), (3143, 3025, Security::Plain));
    }
}
