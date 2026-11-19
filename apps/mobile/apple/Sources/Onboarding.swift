import SwiftUI

/// The intro before the app: Welcome, Choose account, then Notifications once an account is in. Every step stands on
/// the app icon's sky, blue where the words are. Copy matches the Android intro word for word.
struct OnboardingView: View {
    let model: MailModel
    /// The account is in; the intro asks for notifications.
    let notifications: Bool
    let allow: () -> Void
    let notNow: () -> Void
    @Environment(\.webAuthenticationSession) private var browser
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var choosing = Storage.introChoose
    @State private var other = false
    @State private var connecting = false
    @State private var error: String?
    @State private var height: CGFloat = 0
    /// The sky's frame and the bottom of the step's heading, both global, so the blue reaches past the white words.
    @State private var sky = CGRect.zero
    @State private var headBottom: CGFloat = 0

    private enum Step { case welcome, choose, notifications }
    private var step: Step { notifications ? .notifications : choosing ? .choose : .welcome }

    /// Where the solid blue ends, as a fraction of the sky: 24 pt under the heading.
    private var band: CGFloat {
        guard sky.height > 0, headBottom > 0 else { return 0.4 }
        return min(max((headBottom + 24 - sky.minY) / sky.height, 0.2), 0.85)
    }

    var body: some View {
        ZStack {
            Sky(band: band)
                .ignoresSafeArea()
                .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { sky = $0 }
                .animation(Motion.move(reduceMotion), value: band)
            switch step {
            case .welcome: page(.welcome)
            case .choose: page(.choose)
            case .notifications: page(.notifications)
            }
        }
        .animation(Motion.move(reduceMotion), value: step)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .foregroundStyle(Theme.ink)
        // The page changes in place, so VoiceOver starts again at its top.
        .onChange(of: step) { _, _ in AccessibilityNotification.ScreenChanged().post() }
        .sheet(isPresented: $other) { AddAccountView(model: model, providers: false).duoRail() }
    }

    /// One step: Back on Choose account, the content scrolling under large text, its buttons at the foot.
    /// On iPad the column stops at 800 pt, so the buttons stay near the content.
    private func page(_ step: Step) -> some View {
        VStack(spacing: 0) {
            HStack {
                if step == .choose {
                    Button { choosing = false; error = nil } label: { Icon("chevron-left", size: 20) }
                        .foregroundStyle(Theme.skyInk).frame(width: 44, height: 44).contentShape(Rectangle())
                        .accessibilityLabel("Back").disabled(connecting)
                }
            }
            .padding(.horizontal, 10).frame(maxWidth: .infinity, minHeight: 56, alignment: .leading)
            VStack(spacing: 0) {
                ScrollView {
                    VStack(spacing: 0) { content(step) }
                        .padding(.horizontal, 24).padding(.bottom, 24)
                        .frame(maxWidth: 440)
                        .frame(maxWidth: .infinity, minHeight: height, alignment: step == .choose ? .top : .center)
                }
                .scrollBounceBehavior(.basedOnSize)
                .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { height = $0 }
                VStack(spacing: 6) { buttons(step) }
                    .padding(.horizontal, 24).padding(.bottom, 16)
                    .frame(maxWidth: 440)
            }
            .frame(maxHeight: 800).frame(maxHeight: .infinity)
        }
        .transition(.asymmetric(insertion: Motion.slide(step == .welcome ? .leading : .trailing, reduceMotion), removal: .opacity))
    }

    /// The words on the blue: the sky follows their bottom edge.
    private func head(_ shown: Step, @ViewBuilder _ words: () -> some View) -> some View {
        words()
            .foregroundStyle(Theme.skyInk)
            .onGeometryChange(for: CGFloat.self) { $0.frame(in: .global).maxY } action: { if shown == step { headBottom = $0 } }
    }

    @ViewBuilder private func content(_ step: Step) -> some View {
        switch step {
        case .welcome:
            head(.welcome) {
                VStack(spacing: 0) {
                    Image("Brand").resizable().frame(width: 104, height: 104)
                        .clipShape(.rect(cornerRadius: 24, style: .continuous))
                        .shadow(color: .black.opacity(0.18), radius: 20, y: 10)
                        .accessibilityHidden(true)
                    DisplayTitle(text: "lagom mail", size: 44).padding(.top, 28)
                    Text("Calm email for a clear day.").lagomFont(.title, .regular).padding(.top, 8)
                }
                .multilineTextAlignment(.center)
            }
        case .choose:
            head(.choose) {
                VStack(alignment: .leading, spacing: 8) {
                    DisplayTitle(text: "Choose your mail", size: 34)
                    Text("lagom connects straight to your mail server, with nothing in between.").lagomFont(.body)
                }
                .multilineTextAlignment(.leading).frame(maxWidth: .infinity, alignment: .leading)
            }
            card.padding(.top, 28)
        case .notifications:
            head(.notifications) {
                VStack(spacing: 0) {
                    Icon("inbox", size: 30)
                        .padding(22).background(Theme.skyInk.opacity(0.16), in: .rect(cornerRadius: Theme.Radius.card, style: .continuous))
                    DisplayTitle(text: "New mail, when it arrives", size: 30).padding(.top, 24)
                    Text("lagom tells you about new mail in your Inbox. Nothing else.").lagomFont(.body).padding(.top, 8)
                }
                .multilineTextAlignment(.center)
            }
        }
    }

    /// Choose account's card: the browser sign-ins first, then the mail apps that use the mail server form.
    private var card: some View {
        VStack(spacing: 0) {
            VStack(spacing: 10) {
                // Hidden until this build has the provider's client ID, as in Add account. Google asks for its full
                // button text and its "G" on white.
                ForEach([(SignInProvider.google, "Google", "GoogleLogo"), (.microsoft, "Microsoft", "MicrosoftLogo")], id: \.1) { provider, name, logo in
                    if signInAvailable(provider: provider) {
                        Button { Task { await signIn(provider) } } label: {
                            HStack(spacing: 12) {
                                Image(logo).resizable().frame(width: 18, height: 18)
                                    .frame(width: 30, height: 30).background(.white, in: .circle).accessibilityHidden(true)
                                Text("Continue with \(name)")
                            }
                        }
                        .buttonStyle(PillStyle(fill: Theme.subtle, ink: Theme.ink))
                    }
                }
            }
            MonoLabel(text: "Other mail").padding(.top, 22).padding(.bottom, 14)
            HStack(alignment: .top, spacing: 8) {
                tile("iCloud", icon: "cloud")
                tile("Fastmail", icon: "inbox")
                tile("Yahoo", icon: "mail")
                tile("Other (IMAP)", icon: "settings")
            }
            if connecting {
                HStack(spacing: 8) {
                    ProgressView()
                    Text("Signing in…").lagomFont(.small).foregroundStyle(Theme.secondary)
                }
                .accessibilityElement(children: .combine)
                .padding(.top, 18)
            }
            if let error { Text(error).lagomFont(.snippet).foregroundStyle(Theme.danger).multilineTextAlignment(.center).padding(.top, 18) }
            HStack(spacing: 6) {
                Icon("access", size: 14)
                Text("Passwords stay on this device.")
            }
            .lagomFont(.small).foregroundStyle(Theme.secondary)
            .padding(.top, 22)
        }
        .disabled(connecting)
        .padding(20)
        .background(Theme.raised, in: .rect(cornerRadius: Theme.Radius.card, style: .continuous))
        .shadow(color: .black.opacity(0.10), radius: 24, y: 8)
    }

    /// A mail app as an icon tile with its name under it, as Spark lists providers. The address decides the servers.
    private func tile(_ title: String, icon: String) -> some View {
        Button { other = true } label: {
            VStack(spacing: 8) {
                Icon(icon, size: 24).foregroundStyle(Theme.accent)
                    .frame(width: 56, height: 56)
                    .background(Theme.subtle, in: .rect(cornerRadius: Theme.Radius.panel, style: .continuous))
                Text(title).lagomFont(.small, .medium).foregroundStyle(Theme.ink).multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .frame(maxWidth: .infinity).contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    @ViewBuilder private func buttons(_ step: Step) -> some View {
        switch step {
        case .welcome:
            Button("Get started") { choosing = true }.buttonStyle(PillStyle())
            Text("No tracking. Your mail goes only to your server.")
                .lagomFont(.small).foregroundStyle(Theme.secondary).multilineTextAlignment(.center).padding(.top, 6)
        case .choose: EmptyView()
        case .notifications:
            Button("Allow", action: allow).buttonStyle(PillStyle())
            Button("Not now", action: notNow).buttonStyle(PillStyle(fill: .clear, ink: Theme.ink))
        }
    }

    private func signIn(_ provider: SignInProvider) async {
        error = nil
        defer { connecting = false }
        do {
            guard try await model.signIn(provider, browser: browser, exchanging: { connecting = true }) else { return }
            await model.sync()
        } catch { self.error = error.localizedDescription }
    }
}

/// The app icon's sky: solid blue down to `band`, then peach, then warm light (night colours in dark mode).
private struct Sky: View {
    let band: CGFloat

    var body: some View {
        LinearGradient(stops: [.init(color: Theme.skyTop, location: 0), .init(color: Theme.skyTop, location: band),
                               .init(color: Theme.skyMid, location: (band + 1) / 2), .init(color: Theme.skyBottom, location: 1)],
                       startPoint: .top, endPoint: .bottom)
    }
}

/// The intro's one way on: a full-width pill, ink on the light end of the sky, as the site's primary button.
private struct PillStyle: ButtonStyle {
    var fill = Theme.ink
    var ink = Theme.background
    @Environment(\.isEnabled) private var enabled

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .lagomFont(.body, .semibold)
            .foregroundStyle(ink)
            .padding(.horizontal, 20)
            .frame(maxWidth: .infinity, minHeight: 52)
            .background(fill, in: .capsule)
            .contentShape(.capsule)
            .opacity(configuration.isPressed ? 0.8 : enabled ? 1 : 0.5)
    }
}

/// The site's display line: Hauora Bold with its tight tracking, growing with Dynamic Type.
private struct DisplayTitle: View {
    let text: String
    let size: CGFloat
    @ScaledMetric(relativeTo: .largeTitle) private var scale: CGFloat = 1

    var body: some View {
        Text(text).font(.custom(Theme.Face.display, size: size, relativeTo: .largeTitle))
            .tracking(size * scale * Theme.Face.displayTracking)
            .accessibilityAddTraits(.isHeader)
    }
}
