import SwiftUI

/// The intro before the app: Welcome, Choose account, then Notifications once an account is in.
/// The brand of DESIGN_PROMPT.md: paper, ink and one accent; flat, hairlines, small radii; the dictionary entry as hero.
/// Spacing is on an 8 pt grid. Copy matches the Android intro word for word.
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
    /// Welcome's group height, to centre it in the free height.
    @State private var group: CGFloat = 0
    /// Bottom safe-area inset minus top: Welcome centres on the whole screen, not the safe area.
    @State private var inset: CGFloat = 0
    /// Welcome's icon starts where the launch screen drew it (96 pt, centred) and travels into place.
    @State private var settled = false
    @State private var root = CGRect.zero
    @State private var mark = CGRect.zero
    /// Counts primary taps, for one light haptic each.
    @State private var taps = 0

    private enum Step { case welcome, choose, notifications }
    private var step: Step { notifications ? .notifications : choosing ? .choose : .welcome }
    /// Steps move without bounce: the shared 280 ms, or only a fade under Reduce Motion.
    private var move: Animation { reduceMotion ? Motion.fade : .smooth(duration: 0.28) }

    var body: some View {
        ZStack {
            switch step {
            case .welcome: welcome
            case .choose: page(.choose)
            case .notifications: page(.notifications)
            }
        }
        .animation(move, value: step)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { root = $0 }
        .background(Theme.background.ignoresSafeArea())
        .foregroundStyle(Theme.ink)
        .sensoryFeedback(.impact(weight: .light), trigger: taps)
        // The page changes in place, so VoiceOver starts again at its top.
        .onChange(of: step) { _, _ in AccessibilityNotification.ScreenChanged().post() }
        .sheet(isPresented: $other) { AddAccountView(model: model, providers: false).duoRail() }
        .task {
            guard !settled else { return }
            // One frame for the icon's place to be measured, then it travels; under Reduce Motion it is simply there.
            try? await Task.sleep(for: .milliseconds(reduceMotion ? 0 : 300))
            withAnimation(reduceMotion ? Motion.fade : .smooth(duration: 0.45)) { settled = true }
        }
    }

    /// Welcome, as on Mac and Android: one centred group (icon, wordmark, dictionary line, definition, button, caption)
    /// centred on the screen, scrolling under large text. The wordmark is the one heavy element.
    private var welcome: some View {
        ScrollView {
            VStack(spacing: 0) {
                Image("LaunchIcon")
                    .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { mark = $0 }
                    .offset(settled || mark == .zero ? .zero : CGSize(width: root.midX - mark.midX, height: root.midY - mark.midY))
                    .opacity(mark == .zero ? 0 : 1)
                    .accessibilityHidden(true)
                VStack(spacing: 0) {
                    DisplayTitle(text: "lagom", size: .hero).padding(.top, 24)
                    (Text("/ˈlɑːɡɔm/ ") + Text("adv., adj. Swedish.").foregroundStyle(Theme.secondary))
                        .font(.custom(Self.medium, size: 15, relativeTo: .body)).padding(.top, 8)
                        .accessibilityLabel("Pronounced LAH-gom. Adverb, adjective. Swedish.")
                    // One line where it fits; else one sentence a line, so no width leaves a word alone.
                    ViewThatFits(in: .horizontal) {
                        Text("Not too much. Not too little. Just right.").fixedSize()
                        Text("Not too much.\nNot too little.\nJust right.")
                    }
                    .font(.custom(Self.medium, size: 17, relativeTo: .body)).padding(.top, 20)
                    Button { taps += 1; choosing = true } label: {
                        Text("Add account").lagomFont(.base, .medium).frame(width: 280).frame(minHeight: 48)
                    }
                    .buttonStyle(HouseButtonStyle(primary: true)).padding(.top, 32)
                    Text("No tracking. No lagom servers.").lagomFont(.small).foregroundStyle(Theme.secondary).padding(.top, 12)
                }
                .opacity(settled ? 1 : 0)
                // Hidden until the icon settles, so it takes no tap or VoiceOver focus before it shows.
                .allowsHitTesting(settled)
                .accessibilityHidden(!settled)
            }
            .multilineTextAlignment(.center)
            .padding(.horizontal, 24).padding(.vertical, 16)
            .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { group = $0 }
            .padding(.top, max(0, (height - group + inset) / 2))
            .frame(maxWidth: .infinity, minHeight: height, alignment: .top)
        }
        .scrollBounceBehavior(.basedOnSize)
        .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { height = $0 }
        .onGeometryChange(for: CGFloat.self) { $0.safeAreaInsets.bottom - $0.safeAreaInsets.top } action: { inset = $0 }
        .background(Theme.background)
        .transition(.asymmetric(insertion: Motion.slide(.leading, reduceMotion), removal: .opacity))
    }

    /// Hauora Medium: the lightest bundled weight, for Welcome's body lines.
    private static let medium = "Hauora-Medium"

    /// One step: Back on Choose account, the content scrolling under large text, its buttons at the foot.
    /// On iPad the column stops at 800 pt, so the buttons stay near the content.
    private func page(_ step: Step) -> some View {
        VStack(spacing: 0) {
            HStack {
                if step == .choose {
                    IconButton(title: "Back", icon: "chevron-left") { choosing = false; error = nil }.disabled(connecting)
                }
            }
            .padding(.horizontal, 12).frame(maxWidth: .infinity, minHeight: 56, alignment: .leading)
            VStack(spacing: 0) {
                ScrollView {
                    VStack(alignment: .leading, spacing: 0) { content(step) }
                        .multilineTextAlignment(.leading)
                        .padding(.horizontal, 24).padding(.bottom, 32)
                        .frame(maxWidth: 440, alignment: .leading)
                        .frame(maxWidth: .infinity, minHeight: height, alignment: step == .choose ? .top : .center)
                }
                .scrollBounceBehavior(.basedOnSize)
                .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { height = $0 }
                VStack(spacing: 8) { buttons(step) }
                    .padding(.horizontal, 24).padding(.bottom, 16)
                    .frame(maxWidth: 440)
            }
            .frame(maxHeight: 800).frame(maxHeight: .infinity)
        }
        .background(Theme.background)
        .transition(.asymmetric(insertion: Motion.slide(step == .welcome ? .leading : .trailing, reduceMotion), removal: .opacity))
    }

    @ViewBuilder private func content(_ step: Step) -> some View {
        switch step {
        case .welcome: EmptyView()
        case .choose:
            DisplayTitle(text: "Choose your mail", size: .display).padding(.top, 8)
            let providers = [(SignInProvider.google, "Google", "GoogleLogo"), (.microsoft, "Microsoft", "MicrosoftLogo")]
                .filter { signInAvailable(provider: $0.0) }
            Group {
                if !providers.isEmpty {
                    // Hidden until this build has the provider's client ID, as in Add account. Google asks for its full
                    // button text and its "G" on white.
                    rows {
                        ForEach(Array(providers.enumerated()), id: \.offset) { index, item in
                            if index > 0 { hairline }
                            row("Continue with \(item.1)", mark: Image(item.2).resizable().frame(width: 16, height: 16)
                                .frame(width: 32, height: 32).background(.white, in: .circle)
                                .overlay { Circle().strokeBorder(Theme.line) }) { Task { await signIn(item.0) } }
                        }
                    }
                    .padding(.top, 24)
                }
                MonoLabel(text: "Other mail").padding(.top, 32).padding(.bottom, 8)
                // The address decides the servers, so every mail app opens the same form.
                rows {
                    row("iCloud", mark: Icon("mail", size: 18).foregroundStyle(Theme.secondary)) { other = true }
                    hairline
                    row("Fastmail", mark: Icon("mail", size: 18).foregroundStyle(Theme.secondary)) { other = true }
                    hairline
                    row("Yahoo", mark: Icon("mail", size: 18).foregroundStyle(Theme.secondary)) { other = true }
                    hairline
                    row("Other (IMAP)", mark: Icon("mail", size: 18).foregroundStyle(Theme.secondary)) { other = true }
                }
            }
            .disabled(connecting)
            if connecting {
                HStack(spacing: 8) {
                    ProgressView()
                    Text("Signing in…").lagomFont(.small).foregroundStyle(Theme.secondary)
                }
                .accessibilityElement(children: .combine)
                .padding(.top, 16)
            }
            if let error { Text(error).lagomFont(.snippet).foregroundStyle(Theme.danger).padding(.top, 16) }
            HStack(spacing: 8) {
                Icon("access", size: 14)
                Text("Passwords stay on this device.")
            }
            .lagomFont(.small).foregroundStyle(Theme.secondary)
            .accessibilityElement(children: .combine)
            .padding(.top, 16)
        case .notifications:
            Icon("inbox", size: 32).foregroundStyle(Theme.accent)
            DisplayTitle(text: "Notifications", size: .display).padding(.top, 24)
            Text("lagom tells you when new mail arrives in your Inbox. Nothing else.")
                .lagomFont(.body).foregroundStyle(Theme.secondary).padding(.top, 8)
        }
    }

    @ViewBuilder private func buttons(_ step: Step) -> some View {
        switch step {
        case .welcome, .choose: EmptyView()
        case .notifications:
            wide("Allow", icon: "done", primary: true) { taps += 1; allow() }
            wide("Not now", icon: "close", action: notNow)
        }
    }

    /// A house button across the column, 48 pt tall: the step's one way on.
    private func wide(_ title: String, icon: String, primary: Bool = false, action: @escaping () -> Void) -> some View {
        Button(action: action) { ButtonLabel(title: title, icon: icon, primary: primary).frame(maxWidth: .infinity, minHeight: 48) }
            .buttonStyle(HouseButtonStyle(primary: primary))
    }

    /// A list of rows in one hairline frame, like a shopping list.
    private func rows(@ViewBuilder _ content: () -> some View) -> some View {
        VStack(spacing: 0) { content() }
            .overlay { RoundedRectangle(cornerRadius: Theme.Radius.control).strokeBorder(Theme.line) }
            .clipShape(.rect(cornerRadius: Theme.Radius.control))
    }

    /// Between rows, from the text's edge: 16 inset + 32 mark + 16 gap.
    private var hairline: some View { Rectangle().fill(Theme.line).frame(height: 1).padding(.leading, 64) }

    /// One account kind: its mark, the name and a chevron.
    private func row(_ title: String, mark: some View, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 16) {
                mark.frame(width: 32).accessibilityHidden(true)
                Text(title).lagomFont(.body, .medium)
                Spacer(minLength: 8)
                Icon("chevron-right", size: 14).foregroundStyle(Theme.secondary)
            }
            .padding(.horizontal, 16).padding(.vertical, 8).frame(maxWidth: .infinity, minHeight: 56)
            .contentShape(Rectangle())
        }
        .buttonStyle(RowStyle())
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

/// A list row: flat, the subtle fill only while pressed.
private struct RowStyle: ButtonStyle {
    @Environment(\.isEnabled) private var enabled

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .foregroundStyle(Theme.ink)
            .background(configuration.isPressed ? Theme.subtle : .clear)
            .opacity(enabled ? 1 : 0.5)
    }
}

/// The site's display face: Hauora Bold with its tight tracking, growing with Dynamic Type.
private struct DisplayTitle: View {
    let text: String
    let size: TextSize
    var header = true
    @ScaledMetric(relativeTo: .largeTitle) private var scale: CGFloat = 1

    var body: some View {
        // One Text a line: Hauora's own line height is about 1.37; the site, Android and Mac set 1.15.
        VStack(alignment: .leading, spacing: size.points * scale * -0.22) {
            ForEach(text.components(separatedBy: "\n"), id: \.self) { line in
                Text(line).font(.custom(Theme.Face.display, size: size.points, relativeTo: .largeTitle))
                    .tracking(size.points * scale * Theme.Face.displayTracking)
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(header ? .isHeader : [])
    }
}
