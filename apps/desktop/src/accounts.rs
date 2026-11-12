//! Add an account, email first: the worker looks up the provider, then Google or Microsoft
//! sign-in runs in the browser, or the app-password form opens with the servers filled in.
//! "Continue with Google/Microsoft" skips the lookup; "Set up manually" lists the providers.
//! "Sign in" hands the account to the mail worker, which logs in to IMAP and SMTP before it
//! saves anything.

use gpui::{
    div, prelude::*, px, relative, App, Context, DismissEvent, Entity, EventEmitter, FocusHandle,
    Focusable, FontWeight, KeyDownEvent, SharedString, Subscription, Window,
};
use lagom_core::account::{Account, Security, ServerConfig};
use lagom_core::discovery::Discovery;
use lagom_core::oauth;
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
            Provider::Other => "Another provider",
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
            Provider::Outlook => "Microsoft wants an app password here, not your normal password. Some accounts only work with Sign in with Microsoft.",
            Provider::Other => "Use the password from your mail provider. If they offer app passwords, use one.",
            Provider::Gmail => "Google wants an app password here, not your normal password. You make one in your Google account.",
            Provider::ICloud | Provider::Fastmail => "Use an app password, not your normal password. You make one in your account settings.",
        }
    }

    /// Who the row is for, in the words people use for their mail.
    pub fn hint(self) -> &'static str {
        match self {
            Provider::Gmail => "Google mail and Google Workspace",
            Provider::ICloud => "iCloud, me.com and mac.com",
            Provider::Fastmail => "Fastmail",
            Provider::Outlook => "Outlook, Hotmail, Live and Microsoft 365",
            Provider::Other => "Any other email address",
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
        return Err("Enter your password.".into());
    }
    let server = |label: &str, host: &str, port: &str, security| -> Result<ServerConfig, String> {
        let host = host.trim();
        if host.is_empty() {
            return Err(format!("Enter the {label} server."));
        }
        let port = port.trim().parse::<u16>().ok().filter(|p| *p > 0).ok_or(format!("The {label} port must be a number from 1 to 65535."))?;
        let config = ServerConfig { host: host.into(), port, security, username: email.into() };
        config.validate().map_err(|e| match security {
            Security::Plain => format!("The {label} server needs encryption."),
            _ => format!("Check the {label} server: {e}."),
        })?;
        Ok(config)
    };
    let imap = server("incoming mail", &s.imap_host, &s.imap_port, s.imap_security)?;
    let smtp = server("outgoing mail", &s.smtp_host, &s.smtp_port, s.smtp_security)?;
    Account::new(email.into(), imap, smtp).map_err(|_| "That email address is not valid.".into())
}

fn security_label(s: Security) -> &'static str {
    match s {
        Security::Tls => "TLS",
        Security::StartTls => "STARTTLS",
        Security::Plain => "None",
    }
}

/// Where the person is in Add account.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Screen {
    /// Email first, or Continue with Google or Microsoft.
    Start,
    /// "Set up manually": the provider list.
    Pick,
    /// Browser sign-in for one provider.
    Browser(oauth::Provider),
    /// Email, app password and servers.
    Form,
}

#[derive(Clone, PartialEq, Debug)]
enum State {
    Idle,
    /// Looking up the provider of the typed address.
    Discovering,
    Connecting,
    /// Google or Microsoft sign-in runs in the browser.
    SigningIn,
    /// Esc was pressed; the worker has not confirmed the cancel yet.
    Cancelling,
    Failed(String),
}

/// "Sign in" on the form: the worker verifies and saves the account.
pub struct Connect(pub Box<Account>, pub Zeroizing<String>);

/// Start or cancel Google or Microsoft sign-in in the browser; the worker saves the account.
pub enum SignIn { Start(oauth::Provider), Cancel }

/// Look up the provider of this address; the worker answers with `Event::Discovered`.
pub struct Discover(pub String);

pub struct AddAccount {
    screen: Screen,
    state: State,
    provider: Provider,
    /// Accounts exist, so Esc can go back to the inbox.
    can_cancel: bool,
    /// Lowercase addresses already in lagom.
    existing: Vec<String>,
    advanced: bool,
    /// The provider's app-password page from discovery, when it knows one.
    help: Option<String>,
    /// Which OAuth clients this build has.
    available: fn(oauth::Provider) -> bool,
    /// Login names from discovery, for the address they were found for; some servers want
    /// only the part before @.
    usernames: Option<(String, String, String)>,
    focus: FocusHandle,
    /// Tab stops for the buttons and links of the current screen, in order: Return or Space presses them.
    stops: [FocusHandle; 5],
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
impl EventEmitter<SignIn> for AddAccount {}
impl EventEmitter<Discover> for AddAccount {}
impl EventEmitter<DismissEvent> for AddAccount {}

impl Focusable for AddAccount {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn oauth_name(p: oauth::Provider) -> &'static str {
    match p { oauth::Provider::Google => "Google", oauth::Provider::Microsoft => "Microsoft" }
}

/// The provider's sign-in mark, in its own colours as its brand rules ask.
fn logo(p: oauth::Provider, s: f32) -> gpui::Img {
    let path = match p { oauth::Provider::Google => "logos/google.svg", oauth::Provider::Microsoft => "logos/microsoft.svg" };
    gpui::img(gpui::ImageSource::Resource(gpui::Resource::Embedded(path.into()))).size(px(s)).flex_shrink_0()
}

impl AddAccount {
    pub fn new(can_cancel: bool, existing: Vec<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let field = |placeholder: &'static str, masked: bool, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let input = TextInput::new(window, cx).bordered().placeholder(placeholder);
                if masked { input.masked() } else { input }
            })
        };
        let email = field("you@example.com", false, window, cx);
        let password = field("Password", true, window, cx);
        let imap_host = field("imap.example.com", false, window, cx);
        let imap_port = field("993", false, window, cx);
        let smtp_host = field("smtp.example.com", false, window, cx);
        let smtp_port = field("587", false, window, cx);
        let fields = [&email, &password, &imap_host, &imap_port, &smtp_host, &smtp_port]
            .map(|f| cx.subscribe_in(f, window, Self::on_field))
            .into();
        window.focus(&email.focus_handle(cx));
        Self {
            screen: Screen::Start,
            state: State::Idle,
            provider: Provider::Other,
            can_cancel,
            existing: existing.into_iter().map(|e| e.to_lowercase()).collect(),
            advanced: false,
            help: None,
            available: oauth::available,
            usernames: None,
            focus: cx.focus_handle(),
            stops: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
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

    fn busy(&self) -> bool {
        matches!(self.state, State::Discovering | State::Connecting | State::SigningIn | State::Cancelling)
    }

    fn typed_email(&self, cx: &App) -> String {
        self.email.read(cx).text().trim().to_string()
    }

    /// The arrow next to the email field: look up the provider.
    pub fn submit_email(&mut self, cx: &mut Context<Self>) {
        if self.screen != Screen::Start || self.busy() { return; }
        let email = self.typed_email(cx);
        self.state = if !email.split_once('@').is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.')) {
            State::Failed("Enter your full email address.".into())
        } else if self.existing.contains(&email.to_lowercase()) {
            State::Failed(format!("{email} is already in lagom."))
        } else {
            cx.emit(Discover(email));
            State::Discovering
        };
        cx.notify();
    }

    /// The worker's answer to `Discover`. A late answer for another address changes nothing.
    pub fn discovered(&mut self, email: &str, discovery: Discovery, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen != Screen::Start || self.state != State::Discovering || email != self.typed_email(cx) { return; }
        match discovery {
            Discovery::OAuth(p) if (self.available)(p) => self.browser(p, false, window, cx),
            Discovery::OAuth(oauth::Provider::Google) => self.form(Provider::Gmail, window, cx),
            Discovery::OAuth(oauth::Provider::Microsoft) => self.form(Provider::Outlook, window, cx),
            Discovery::Password { preset, help_url } => {
                let p = Provider::ALL.into_iter().find(|p| Servers::for_provider(*p).imap_host == preset.imap.host).unwrap_or(Provider::Other);
                self.form(p, window, cx);
                self.advanced = false;
                for (field, text) in [(&self.imap_host, preset.imap.host.clone()), (&self.imap_port, preset.imap.port.to_string()), (&self.smtp_host, preset.smtp.host.clone()), (&self.smtp_port, preset.smtp.port.to_string())] {
                    field.update(cx, |f, cx| f.set_text(&text, cx));
                }
                (self.imap_security, self.smtp_security) = (preset.imap.security, preset.smtp.security);
                self.help = help_url;
                self.usernames = Some((email.to_string(), preset.imap.username, preset.smtp.username));
            }
            Discovery::Manual => self.form(Provider::Other, window, cx),
        }
    }

    /// Browser sign-in for `p`; `start` opens the browser at once, as "Continue with Google" does.
    fn browser(&mut self, p: oauth::Provider, start: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy() && self.state != State::Discovering { return; }
        self.screen = Screen::Browser(p);
        self.state = State::Idle;
        window.focus(&self.focus);
        if start { self.sign_in(window, cx); }
        cx.notify();
    }

    /// A row in the provider list: browser sign-in where this build can, else the form.
    pub fn pick(&mut self, p: Provider, window: &mut Window, cx: &mut Context<Self>) {
        match p {
            Provider::Gmail if (self.available)(oauth::Provider::Google) => self.browser(oauth::Provider::Google, false, window, cx),
            Provider::Outlook if (self.available)(oauth::Provider::Microsoft) => self.browser(oauth::Provider::Microsoft, false, window, cx),
            _ => self.form(p, window, cx),
        }
    }

    /// The app-password form for `p`, with its server presets and the address typed so far.
    fn form(&mut self, p: Provider, window: &mut Window, cx: &mut Context<Self>) {
        self.provider = p;
        self.advanced = p == Provider::Other;
        self.help = None;
        self.usernames = None;
        let s = Servers::for_provider(p);
        for (field, text) in [(&self.imap_host, &s.imap_host), (&self.imap_port, &s.imap_port), (&self.smtp_host, &s.smtp_host), (&self.smtp_port, &s.smtp_port)] {
            field.update(cx, |f, cx| f.set_text(text, cx));
        }
        (self.imap_security, self.smtp_security) = (s.imap_security, s.smtp_security);
        self.screen = Screen::Form;
        self.state = State::Idle;
        let next = if self.typed_email(cx).is_empty() { &self.email } else { &self.password };
        window.focus(&next.focus_handle(cx));
        cx.notify();
    }

    /// Test hook (`LAGOM_TEST_ACCOUNT`): fills the form as typing would.
    pub fn fill(&mut self, email: &str, password: &str, s: &Servers, window: &mut Window, cx: &mut Context<Self>) {
        self.form(Provider::Other, window, cx);
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
        if self.screen != Screen::Form || self.busy() { return; }
        let password = Zeroizing::new(self.password.read(cx).text().to_string());
        match build(self.email.read(cx).text(), &password, &self.servers(cx)) {
            Ok(account) if self.existing.contains(&account.email.to_lowercase()) => self.state = State::Failed(format!("{} is already in lagom.", account.email)),
            Ok(mut account) => {
                if let Some((_, imap, smtp)) = self.usernames.clone().filter(|(email, ..)| *email == account.email) {
                    (account.imap.username, account.smtp.username) = (imap, smtp);
                }
                self.state = State::Connecting;
                cx.emit(Connect(Box::new(account), password));
            }
            Err(e) => self.state = State::Failed(e),
        }
        cx.notify();
    }

    pub fn sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Screen::Browser(p) = self.screen else { return };
        if !matches!(self.state, State::Idle | State::Failed(_)) { return; }
        // The app-password link hides while waiting; Esc must still reach this view to cancel.
        window.focus(&self.focus);
        self.state = State::SigningIn;
        cx.emit(SignIn::Start(p));
        cx.notify();
    }

    /// The worker confirmed that the browser sign-in ended without an account.
    // shortcut: events carry no attempt id, so leaving the page while Cancelling and starting again can still be reset by the late confirm; add an id if that shows up.
    pub fn cancelled(&mut self, cx: &mut Context<Self>) {
        if matches!(self.state, State::SigningIn | State::Cancelling) {
            self.state = State::Idle;
            cx.notify();
        }
    }

    /// The worker could not log in.
    pub fn failed(&mut self, error: String, cx: &mut Context<Self>) {
        // The worker still confirms a pending cancel; leaving Cancelling now would let that reset a new attempt.
        if self.state == State::Cancelling { return; }
        self.state = State::Failed(error);
        cx.notify();
    }

    fn on_field(&mut self, field: &Entity<TextInput>, e: &TextInputEvent, window: &mut Window, cx: &mut Context<Self>) {
        match e {
            TextInputEvent::Enter if *field == self.email && self.screen == Screen::Start => self.submit_email(cx),
            TextInputEvent::Enter if *field == self.password && !self.advanced => self.connect(cx),
            TextInputEvent::Enter if *field == self.smtp_port => self.connect(cx),
            TextInputEvent::Enter => window.focus_next(),
            TextInputEvent::Escape => self.back(window, cx),
            // A new address drops the old answer or error; a lookup still running is ignored when it lands.
            TextInputEvent::Changed if *field == self.email && self.screen == Screen::Start && matches!(self.state, State::Failed(_) | State::Discovering) => {
                self.state = State::Idle;
                cx.notify();
            }
            TextInputEvent::Changed => {}
        }
    }

    pub(crate) fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match (self.screen, &self.state) {
            (_, State::SigningIn) => {
                self.state = State::Cancelling;
                cx.emit(SignIn::Cancel);
                cx.notify();
            }
            (_, State::Connecting | State::Cancelling) => {}
            (Screen::Start, State::Discovering) => {
                self.state = State::Idle;
                cx.notify();
            }
            (Screen::Start, _) => if self.can_cancel { cx.emit(DismissEvent) },
            _ => {
                self.screen = Screen::Start;
                self.state = State::Idle;
                window.focus(&self.email.focus_handle(cx));
                cx.notify();
            }
        }
    }

    /// 1–5 pick a provider; the submit shortcut continues; Esc goes back.
    fn on_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        if matches!(k.key.as_str(), "enter" | "space") && k.modifiers == Default::default() {
            if let Some(i) = self.stops.iter().position(|s| s.is_focused(window)) {
                self.press(i, window, cx);
                cx.stop_propagation();
                return;
            }
        }
        match k.key.as_str() {
            // The text fields bind Tab themselves; this moves on from a focused button or link.
            "tab" if k.modifiers.shift => window.focus_prev(),
            "tab" => window.focus_next(),
            "escape" => self.back(window, cx),
            "enter" if crate::shortcuts::submit().matches(k) || (k.modifiers == Default::default() && matches!(self.screen, Screen::Browser(_))) => match self.screen {
                Screen::Start => self.submit_email(cx),
                Screen::Browser(_) => self.sign_in(window, cx),
                Screen::Form => self.connect(cx),
                Screen::Pick => return,
            },
            n if self.screen == Screen::Pick && !k.modifiers.platform => {
                let Some(p) = n.parse::<usize>().ok().and_then(|i| Provider::ALL.get(i.wrapping_sub(1))) else { return };
                self.pick(*p, window, cx);
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    /// The `i`th button or link of this screen, as a click on it.
    fn press(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        match self.screen {
            Screen::Start => match self.start_choices().get(i) {
                Some(Some(p)) => self.browser(*p, true, window, cx),
                Some(None) => self.manual(window, cx),
                None => {}
            },
            Screen::Browser(p) if i == 0 => self.form(if p == oauth::Provider::Google { Provider::Gmail } else { Provider::Outlook }, window, cx),
            Screen::Pick => if let Some(p) = Provider::ALL.get(i) { self.pick(*p, window, cx) },
            _ => {}
        }
    }

    /// Start's buttons under the field: each OAuth client this build has, then "Choose your provider instead".
    fn start_choices(&self) -> Vec<Option<oauth::Provider>> {
        [oauth::Provider::Google, oauth::Provider::Microsoft].into_iter().filter(|p| (self.available)(*p)).map(Some).chain([None]).collect()
    }

    fn manual(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy() { return; }
        self.screen = Screen::Pick;
        self.state = State::Idle;
        window.focus(&self.stops[0]);
        cx.notify();
    }

    /// A tab stop: tracks its focus handle and shows a ring while focused.
    fn stop(&self, el: gpui::Stateful<gpui::Div>, i: usize, t: &Theme, window: &Window) -> gpui::Stateful<gpui::Div> {
        let focused = self.stops[i].is_focused(window);
        el.track_focus(&self.stops[i]).when(focused, |d| d.border_color(t.accent))
    }

    fn status(&self, t: &Theme) -> Option<gpui::Div> {
        let note = |text: String| div().text_size(px(size::SNIPPET)).text_color(t.ink2).child(text);
        match &self.state {
            State::Idle => None,
            State::Discovering => Some(note("Checking your address…".into())),
            State::Connecting => Some(note("Signing in…".into())),
            State::SigningIn => Some(note("Finish signing in in your browser, then come back here.".into())),
            State::Cancelling => Some(note("Cancelling…".into())),
            State::Failed(e) => Some(div().text_size(px(size::SNIPPET)).text_color(t.danger).child(e.clone())),
        }
    }

    fn link(t: &Theme, id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
        // A clear border keeps the layout still when the focus ring appears.
        div().id(id).px(px(6.)).py(px(2.)).rounded(px(radius::KEY)).border_1().border_color(gpui::transparent_black())
            .text_size(px(size::SNIPPET)).text_color(t.ink2).cursor_pointer().hover(|s| s.underline()).child(label)
    }

    fn start(&self, t: &Theme, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let clients: Vec<_> = self.start_choices().into_iter().flatten().collect();
        let manual = clients.len();
        let arrow = div()
            .id("email-continue")
            .debug_selector(|| "email-continue".into())
            .flex_shrink_0()
            .size(px(32.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(radius::CONTROL))
            .bg(t.accent)
            .hover(|s| s.opacity(0.92))
            .active(|s| s.opacity(0.8))
            .on_click(cx.listener(|this, _, _, cx| this.submit_email(cx)))
            .child(icon("icons/next.svg", t.accent_ink, 16.));
        div()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(24.))
            .child(crate::main_window::app_mark(56.))
            .child(div().flex().flex_col().items_center().gap(px(8.)).text_center()
                .child(headline("Add your mail."))
                .child(div().text_size(px(size::TITLE)).text_color(t.ink2).line_height(relative(1.4))
                    .child("lagom talks to your provider directly. Nothing goes through us.")))
            .child(div().w_full().flex().flex_col().gap(px(8.))
                .child(div().w_full().flex().items_center().gap(px(8.)).child(div().flex_1().min_w(px(0.)).child(self.email.clone())).child(arrow))
                .children(self.status(t)))
            .when(!clients.is_empty(), |d| {
                d.child(div().w_full().flex().items_center().gap(px(12.))
                    .child(div().flex_1().h(px(1.)).bg(t.line))
                    .child(mono("or", t.ink2))
                    .child(div().flex_1().h(px(1.)).bg(t.line)))
                .child(div().w_full().flex().flex_col().gap(px(8.)).children(clients.into_iter().enumerate().map(|(i, p)| {
                    self.stop(pressable(div().id(("continue-with", p as usize)), t, false), i, t, window)
                        .debug_selector(move || format!("continue-with-{}", oauth_name(p)))
                        .w_full()
                        .h(px(40.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(10.))
                        .rounded(px(radius::CONTROL))
                        .border_1()
                        .bg(t.bg)
                        .font_weight(FontWeight::MEDIUM)
                        .when(!self.stops[i].is_focused(window), |d| d.border_color(t.line))
                        .on_click(cx.listener(move |this, _, window, cx| this.browser(p, true, window, cx)))
                        .child(logo(p, 18.))
                        .child(format!("Continue with {}", oauth_name(p)))
                })))
            })
            .child(self.stop(Self::link(t, "manual", "Choose your provider instead"), manual, t, window)
                .on_click(cx.listener(|this, _, window, cx| this.manual(window, cx))))
    }

    fn browser_view(&self, p: oauth::Provider, t: &Theme, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let name = oauth_name(p);
        let (title, wait) = match p {
            oauth::Provider::Google => ("Link your Google account", "Waiting for Google…"),
            oauth::Provider::Microsoft => ("Link your Microsoft account", "Waiting for Microsoft…"),
        };
        let waiting = matches!(self.state, State::SigningIn | State::Cancelling);
        div()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(24.))
            .text_center()
            .child(logo(p, 56.))
            .child(div().flex().flex_col().items_center().gap(px(8.))
                .child(headline(title))
                .child(div().text_size(px(size::TITLE)).text_color(t.ink2).line_height(relative(1.4))
                    .child(format!("lagom opens {name} in your browser. Sign in there, then come back here."))))
            .child(button(t, if waiting { wait } else { "Continue in browser" }, Some(t.accent))
                .id("oauth-sign-in")
                .w_full()
                .h(px(40.))
                .justify_center()
                .on_click(cx.listener(|this, _, window, cx| this.sign_in(window, cx))))
            .children(self.status(t))
            .when(!waiting, |d| d.child(self.stop(Self::link(t, "use-app-password", "Use an app password instead"), 0, t, window).on_click(cx.listener(move |this, _, window, cx| {
                this.form(if p == oauth::Provider::Google { Provider::Gmail } else { Provider::Outlook }, window, cx);
            }))))
    }

    fn picker(&self, t: &Theme, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let mut list = div().w_full().flex().flex_col().gap(px(2.)).pt(px(20.)).border_t_1().border_color(t.line);
        for (i, p) in Provider::ALL.into_iter().enumerate() {
            list = list.child(
                self.stop(pressable(div().id(("provider", i)), t, false).border_1().border_color(gpui::transparent_black()), i, t, window)
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .h(px(48.))
                    .px(px(12.))
                    .rounded(px(radius::CONTROL))
                    .on_click(cx.listener(move |this, _, window, cx| this.pick(p, window, cx)))
                    .child(icon("icons/account-add.svg", t.ink2, 16.))
                    .child(div().flex_1().text_size(px(size::BODY)).font_weight(FontWeight::MEDIUM).child(p.name()))
                    .child(div().text_size(px(size::SNIPPET)).text_color(t.ink2).child(p.hint())),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(headline("Who is your mail with?"))
            .child(list)
    }

    fn form_view(&self, t: &Theme, cx: &mut Context<Self>) -> gpui::Div {
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
        let help: Option<SharedString> = self.help.clone().map(Into::into).or(p.help().map(Into::into));
        div()
            .flex()
            .flex_col()
            .gap(px(20.))
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
                            .when_some(help, |d, url| {
                                d.child(
                                    div()
                                        .id("help")
                                        .flex()
                                        .text_color(t.accent)
                                        .cursor_pointer()
                                        .hover(|s| s.underline())
                                        .on_click(move |_, _, cx| cx.open_url(&url))
                                        .gap(px(6.)).child(icon("icons/external.svg", t.ink2, 16.)).child("How to make an app password"),
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
                    .child("Advanced"),
            )
            .when(self.advanced, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .child(server("Incoming", &self.imap_host, &self.imap_port, self.imap_security, true, cx))
                        .child(server("Outgoing", &self.smtp_host, &self.smtp_port, self.smtp_security, false, cx)),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(16.))
                    .pt(px(8.))
                    .child(
                        button(t, if self.state == State::Connecting { "Signing in" } else { "Sign in" }, Some(t.accent))
                            .on_click(cx.listener(|this, _, _, cx| this.connect(cx))),
                    )
                    .children(self.status(t)),
            )
    }
}

fn headline(text: impl Into<SharedString>) -> gpui::Div {
    div().text_size(px(size::DISPLAY)).font_weight(FontWeight::BOLD).child(text.into())
}

impl Render for AddAccount {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::for_appearance(window.appearance(), DEFAULT_ACCENT);
        // Start and the browser step are short and centred, as a sign-in sheet; the forms read top-down.
        let centred = matches!(self.screen, Screen::Start | Screen::Browser(_));
        let content = match self.screen {
            Screen::Start => self.start(&t, window, cx),
            Screen::Browser(p) => self.browser_view(p, &t, window, cx),
            Screen::Pick => self.picker(&t, window, cx),
            Screen::Form => self.form_view(&t, cx),
        };
        let nav = match self.screen {
            Screen::Start if self.can_cancel => Some(icon_button(&t, "close", "icons/close.svg", "Close · Esc")),
            Screen::Start => None,
            _ => Some(icon_button(&t, "back", "icons/previous.svg", "Back · Esc")),
        };
        div()
            .id("add-account")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .px(px(24.))
            .py(px(24.))
            .text_color(t.ink)
            .font_family(font::ui())
            .text_size(px(size::BASE))
            .child(div().w_full().h(px(32.)).flex().children(nav.map(|b| b.on_click(cx.listener(|this, _, window, cx| this.back(window, cx))))))
            .child(div().w_full().flex_1().flex().flex_col().items_center().when(centred, |d| d.justify_center()).pb(px(32.))
                .child(div().w_full().max_w(px(if centred { 400. } else { 560. })).when(!centred, |d| d.pt(px(24.))).child(content)))
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

    fn events<E: 'static>(view: &Entity<AddAccount>, cx: &mut gpui::VisualTestContext, f: fn(&E) -> String) -> std::rc::Rc<std::cell::RefCell<Vec<String>>> where AddAccount: EventEmitter<E> {
        let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let log = sent.clone();
        cx.update(|_, cx| cx.subscribe(view, move |_, e: &E, _| log.borrow_mut().push(f(e))).detach());
        sent
    }

    #[gpui::test]
    fn continue_with_google_starts_and_esc_cancels_it(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| AddAccount { available: |_| true, ..AddAccount::new(true, vec![], window, cx) });
        let sent = events(&view, cx, |e: &SignIn| match e { SignIn::Start(p) => oauth_name(*p).into(), SignIn::Cancel => "cancel".into() });
        view.update_in(cx, |v, window, cx| { v.sign_in(window, cx); v.connect(cx); });
        assert!(view.read_with(cx, |v, _| v.screen == Screen::Start && v.state == State::Idle), "no sign-in or connect from Start");
        view.update_in(cx, |v, window, cx| { v.browser(oauth::Provider::Google, true, window, cx); v.sign_in(window, cx); v.connect(cx); });
        assert!(view.update_in(cx, |v, window, _| v.focus.is_focused(window) && v.state == State::SigningIn));
        view.update_in(cx, |v, window, cx| { v.back(window, cx); v.back(window, cx); v.sign_in(window, cx); v.failed("late".into(), cx); });
        assert!(view.read_with(cx, |v, _| v.state == State::Cancelling), "a new attempt waits for the worker's cancel");
        view.update_in(cx, |v, window, cx| { v.cancelled(cx); window.focus(&v.stops[0]); v.sign_in(window, cx); });
        assert!(view.update_in(cx, |v, window, _| v.focus.is_focused(window)), "waiting moves focus off the hidden link");
        assert_eq!(*sent.borrow(), ["Google", "cancel", "Google"]);
        view.update(cx, |v, cx| v.failed("denied".into(), cx));
        assert!(view.read_with(cx, |v, _| v.state == State::Failed("denied".into())));
        // The provider list opens browser sign-in where the build has a client, else the form.
        view.update_in(cx, |v, window, cx| { v.back(window, cx); v.pick(Provider::Outlook, window, cx); });
        assert_eq!(view.read_with(cx, |v, _| v.screen), Screen::Browser(oauth::Provider::Microsoft));
        view.update_in(cx, |v, window, cx| { v.available = |p| p == oauth::Provider::Google; v.pick(Provider::Outlook, window, cx); });
        assert_eq!(view.read_with(cx, |v, _| (v.screen, v.provider)), (Screen::Form, Provider::Outlook));
    }

    #[gpui::test]
    fn email_first_follows_discovery_and_stops_duplicates(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| AddAccount { available: |p| p == oauth::Provider::Google, ..AddAccount::new(true, vec!["Me@Example.com".into()], window, cx) });
        let asked = events(&view, cx, |Discover(email): &Discover| email.clone());
        // Typing and the arrow are separate updates, as for a person: the field's Changed event lands in between.
        let typed = |text: &'static str, view: &Entity<AddAccount>, cx: &mut gpui::VisualTestContext| {
            view.update(cx, |v, cx| { v.email.update(cx, |f, cx| f.set_text(text, cx)); v.state = State::Idle; });
            view.update(cx, |v, cx| v.submit_email(cx));
        };
        typed("me", &view, cx);
        assert!(view.read_with(cx, |v, _| v.state == State::Failed("Enter your full email address.".into())));
        typed(" me@example.com ", &view, cx);
        assert!(view.read_with(cx, |v, _| v.state == State::Failed("me@example.com is already in lagom.".into())));
        assert!(asked.borrow().is_empty());
        // Google with a client: the browser step, without opening the browser yet.
        typed("a@gmail.com", &view, cx);
        view.update_in(cx, |v, window, cx| v.discovered("b@gmail.com", Discovery::Manual, window, cx));
        assert_eq!(view.read_with(cx, |v, _| (v.screen, v.state.clone())), (Screen::Start, State::Discovering), "an answer for another address changes nothing");
        view.update_in(cx, |v, window, cx| v.discovered("a@gmail.com", Discovery::OAuth(oauth::Provider::Google), window, cx));
        assert_eq!(view.read_with(cx, |v, _| (v.screen, v.state.clone())), (Screen::Browser(oauth::Provider::Google), State::Idle));
        // Microsoft without a client: the app-password form for Outlook.
        view.update_in(cx, |v, window, cx| v.back(window, cx));
        typed("a@outlook.com", &view, cx);
        view.update_in(cx, |v, window, cx| v.discovered("a@outlook.com", Discovery::OAuth(oauth::Provider::Microsoft), window, cx));
        assert_eq!(view.read_with(cx, |v, _| (v.screen, v.provider)), (Screen::Form, Provider::Outlook));
        // A preset fills the servers and keeps the provider's help page.
        view.update_in(cx, |v, window, cx| v.back(window, cx));
        typed("a@corp.example", &view, cx);
        let config = |host: &str, port, security| ServerConfig { host: host.into(), port, security, username: "a@corp.example".into() };
        let preset = lagom_core::discovery::Preset { name: "Corp".into(), imap: ServerConfig { username: "a".into(), ..config("mail.corp.example", 993, Security::Tls) }, smtp: config("smtp.corp.example", 465, Security::Tls) };
        view.update_in(cx, |v, window, cx| v.discovered("a@corp.example", Discovery::Password { preset, help_url: Some("https://help.corp.example".into()) }, window, cx));
        view.read_with(cx, |v, cx| {
            assert_eq!((v.screen, v.provider, v.help.as_deref()), (Screen::Form, Provider::Other, Some("https://help.corp.example")));
            let s = v.servers(cx);
            assert_eq!((s.imap_host.as_str(), s.smtp_port.as_str(), s.smtp_security), ("mail.corp.example", "465", Security::Tls));
        });
        assert_eq!(*asked.borrow(), ["a@gmail.com", "a@outlook.com", "a@corp.example"]);
        // Discovery's login names reach the account, but only for the address they were found for.
        let users = events(&view, cx, |Connect(a, _): &Connect| format!("{} {}", a.imap.username, a.smtp.username));
        view.update(cx, |v, cx| v.password.update(cx, |f, cx| f.set_text("pw", cx)));
        view.update(cx, |v, cx| v.connect(cx));
        view.update(cx, |v, cx| { v.state = State::Idle; v.email.update(cx, |f, cx| f.set_text("b@corp.example", cx)); });
        view.update(cx, |v, cx| v.connect(cx));
        assert_eq!(*users.borrow(), ["a a@corp.example", "b@corp.example b@corp.example"]);
        // Editing the address while the lookup runs frees the arrow again.
        view.update_in(cx, |v, window, cx| { v.state = State::Idle; v.back(window, cx); });
        typed("c@corp.example", &view, cx);
        view.update(cx, |v, cx| v.email.update(cx, |f, cx| f.set_text("d@corp.example", cx)));
        assert!(view.read_with(cx, |v, _| v.state == State::Idle));
        // Tab walks from the field through the choices; Return presses the focused one.
        cx.update(|_, cx| crate::text_input::bind_keys(cx));
        view.update_in(cx, |v, window, cx| { window.activate_window(); window.focus(&v.email.focus_handle(cx)) });
        cx.run_until_parked();
        cx.simulate_keystrokes("tab");
        assert!(view.update_in(cx, |v, window, _| v.stops[0].is_focused(window)), "Tab from the field reaches Continue with Google");
        cx.simulate_keystrokes("tab");
        assert!(view.update_in(cx, |v, window, _| v.stops[1].is_focused(window)), "Tab from a button reaches the next choice");
        cx.simulate_keystrokes("shift-tab");
        assert!(view.update_in(cx, |v, window, _| v.stops[0].is_focused(window)));
        // Return on a focused choice presses it: here Google is the only client, then the provider list.
        view.update_in(cx, |v, window, cx| v.press(1, window, cx));
        assert!(view.update_in(cx, |v, window, _| v.screen == Screen::Pick && v.stops[0].is_focused(window)));
        view.update_in(cx, |v, window, cx| v.press(0, window, cx));
        assert_eq!(view.read_with(cx, |v, _| v.screen), Screen::Browser(oauth::Provider::Google));
        view.update_in(cx, |v, window, cx| v.press(0, window, cx));
        assert_eq!(view.read_with(cx, |v, _| (v.screen, v.provider)), (Screen::Form, Provider::Gmail));
        // The form refuses a duplicate too, whatever its case.
        view.update(cx, |v, cx| { v.email.update(cx, |f, cx| f.set_text("ME@example.com", cx)); v.password.update(cx, |f, cx| f.set_text("pw", cx)); v.connect(cx); });
        assert!(view.read_with(cx, |v, _| v.state == State::Failed("ME@example.com is already in lagom.".into())));
    }

    #[gpui::test]
    fn account_controls_render_icons_and_submit_validates_without_a_worker(cx: &mut gpui::TestAppContext) {
        cx.update(crate::text_input::bind_keys);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let mut view = AddAccount::new(true, vec![], window, cx);
            view.pick(Provider::Other, window, cx);
            view
        });
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        for (label, path) in [("Sign in", "connect"), ("sec-false-STARTTLS", "access"), ("sec-false-None", "close")] {
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
        assert_eq!(build("me@gmail.com", "", &gmail).unwrap_err(), "Enter your password.");
        assert_eq!(build("bad address@x.se", "pw", &gmail).unwrap_err(), "That email address is not valid.");
        let other = Servers::for_provider(Provider::Other);
        assert_eq!(build("me@x.se", "pw", &other).unwrap_err(), "Enter the incoming mail server.");
        let mut s = local();
        s.imap_port = "99999".into();
        assert_eq!(build("me@x.se", "pw", &s).unwrap_err(), "The incoming mail port must be a number from 1 to 65535.");
        s = local();
        s.smtp_host = "smtp.x.se".into();
        assert_eq!(build("me@x.se", "pw", &s).unwrap_err(), "The outgoing mail server needs encryption.");
        let a = build("test@lagom.test", "pw", &local()).unwrap();
        assert_eq!((a.imap.port, a.smtp.port, a.smtp.security), (3143, 3025, Security::Plain));
    }
}
