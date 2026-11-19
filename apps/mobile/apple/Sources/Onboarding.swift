import SwiftUI

/// The intro before the app: Welcome, Choose account, then Notifications once an account is in.
/// Copy matches the Android intro word for word; the voice is the store text's.
struct OnboardingView: View {
    let model: MailModel
    /// The account is in; the intro asks for notifications.
    let notifications: Bool
    let allow: () -> Void
    let notNow: () -> Void
    @Environment(\.webAuthenticationSession) private var browser
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var choosing = false
    @State private var other = false
    @State private var connecting = false
    @State private var error: String?
    @State private var height: CGFloat = 0

    private enum Step { case welcome, choose, notifications }
    private var step: Step { notifications ? .notifications : choosing ? .choose : .welcome }

    var body: some View {
        ZStack {
            switch step {
            case .welcome: page(.welcome)
            case .choose: page(.choose)
            case .notifications: page(.notifications)
            }
        }
        .animation(Motion.move(reduceMotion), value: step)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.background.ignoresSafeArea())
        .foregroundStyle(Theme.ink)
        // The page changes in place, so VoiceOver starts again at its top.
        .onChange(of: step) { _, _ in AccessibilityNotification.ScreenChanged().post() }
        .sheet(isPresented: $other) { AddAccountView(model: model, providers: false).duoRail() }
    }

    /// One step: Back on Choose account, the content centred and scrolling under large text, its buttons at the foot.
    /// On iPad the column stops at 800 pt, so the buttons stay near the content.
    private func page(_ step: Step) -> some View {
        VStack(spacing: 0) {
            HStack {
                if step == .choose {
                    IconButton(title: "Back", icon: "chevron-left") { choosing = false; error = nil }.disabled(connecting)
                }
            }
            .padding(.horizontal, 10).frame(maxWidth: .infinity, minHeight: 56, alignment: .leading)
            VStack(spacing: 0) {
                ScrollView {
                    VStack(spacing: 0) { content(step) }
                        .multilineTextAlignment(.center)
                        .padding(.horizontal, 24).padding(.bottom, 32)
                        .frame(maxWidth: 440)
                        .frame(maxWidth: .infinity, minHeight: height)
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
        case .welcome:
            Image("Brand").resizable().frame(width: 88, height: 88)
                .clipShape(.rect(cornerRadius: 20, style: .continuous))
                .accessibilityHidden(true)
            DisplayTitle(text: "lagom mail", size: 40).padding(.top, 24)
            Text("Calm email for a clear day.").lagomFont(.title, .regular).foregroundStyle(Theme.secondary).padding(.top, 6)
            VStack(alignment: .leading, spacing: 20) {
                point(icon: "done", "Done, Reply or Later", "Each message gets one answer. The list says how many are left.")
                point(icon: "sun", "Today in one sentence", "What is unread, what is due and what is next.")
                point(icon: "access", "Private by default", "No tracking and no ads. Your mail goes only to your own server.")
            }
            .padding(.top, 40)
        case .choose:
            DisplayTitle(text: "Choose your mail", size: 30)
            Text("lagom connects straight to your mail server, with nothing in between.")
                .lagomFont(.body).foregroundStyle(Theme.secondary).padding(.top, 8)
            VStack(spacing: 10) {
                // Hidden until this build has the provider's client ID, as in Add account.
                ForEach([(SignInProvider.google, "Google", "GoogleLogo"), (.microsoft, "Microsoft", "MicrosoftLogo")], id: \.1) { provider, name, logo in
                    if signInAvailable(provider: provider) {
                        choice("Continue with \(name)", mark: Image(logo).resizable().frame(width: 18, height: 18), tile: .white) { Task { await signIn(provider) } }
                    }
                }
                choice("iCloud, Fastmail and other (IMAP)", mark: Icon("mail", size: 18).foregroundStyle(Theme.secondary), tile: Theme.subtle) { other = true }
            }
            .disabled(connecting)
            .padding(.top, 32)
            if connecting {
                HStack(spacing: 8) {
                    ProgressView()
                    Text("Signing in…").lagomFont(.small).foregroundStyle(Theme.secondary)
                }
                .accessibilityElement(children: .combine)
                .padding(.top, 16)
            }
            if let error { Text(error).lagomFont(.snippet).foregroundStyle(Theme.danger).padding(.top, 16) }
            HStack(spacing: 6) {
                Icon("access", size: 14)
                Text("Passwords stay on this device.")
            }
            .lagomFont(.small).foregroundStyle(Theme.secondary)
            .padding(.top, 24)
        case .notifications:
            Icon("inbox", size: 30).foregroundStyle(Theme.accent)
                .padding(22).background(Theme.subtle, in: .rect(cornerRadius: Theme.Radius.card, style: .continuous))
            DisplayTitle(text: "New mail, when it arrives", size: 30).padding(.top, 24)
            Text("lagom tells you about new mail in your Inbox. Nothing else.")
                .lagomFont(.body).foregroundStyle(Theme.secondary).padding(.top, 8)
        }
    }

    @ViewBuilder private func buttons(_ step: Step) -> some View {
        switch step {
        case .welcome: wide("Add account", icon: "account-add", primary: true) { choosing = true }
        case .choose: EmptyView()
        case .notifications:
            wide("Allow", icon: "done", primary: true, action: allow)
            wide("Not now", icon: "close", action: notNow)
        }
    }

    /// A house button across the column, 50 pt tall: the step's one way on.
    private func wide(_ title: String, icon: String, primary: Bool = false, action: @escaping () -> Void) -> some View {
        Button(action: action) { ButtonLabel(title: title, icon: icon, primary: primary).frame(maxWidth: .infinity, minHeight: 50) }
            .buttonStyle(HouseButtonStyle(primary: primary))
    }

    /// One reason to stay: a line icon in a quiet tile, a short title and one sentence.
    private func point(icon: String, _ title: String, _ detail: String) -> some View {
        HStack(alignment: .top, spacing: 14) {
            Icon(icon, size: 20).foregroundStyle(Theme.accent)
                .padding(10).background(Theme.subtle, in: .rect(cornerRadius: Theme.Radius.control, style: .continuous))
            VStack(alignment: .leading, spacing: 2) {
                Text(title).lagomFont(.base, .semibold)
                Text(detail).lagomFont(.snippet).foregroundStyle(Theme.secondary).fixedSize(horizontal: false, vertical: true)
            }
            .padding(.top, 1)
        }
        .multilineTextAlignment(.leading)
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .combine)
    }

    /// One account kind, as desktop's "Continue with" buttons: the mark in a round tile, the name and a chevron.
    /// Google asks for its "G" on white, so provider marks sit on white in dark mode too.
    private func choice(_ title: String, mark: some View, tile: Color, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 14) {
                mark.frame(width: 34, height: 34).background(tile, in: .circle).overlay { Circle().strokeBorder(Theme.line) }.accessibilityHidden(true)
                Text(title).lagomFont(.body, .medium).multilineTextAlignment(.leading)
                Spacer(minLength: 8)
                Icon("chevron-right", size: 14).foregroundStyle(Theme.secondary)
            }
            .padding(.horizontal, 16).frame(maxWidth: .infinity, minHeight: 56)
            .background(Theme.background, in: .rect(cornerRadius: Theme.Radius.panel, style: .continuous))
            .overlay { RoundedRectangle(cornerRadius: Theme.Radius.panel, style: .continuous).strokeBorder(Theme.line) }
            .contentShape(.rect(cornerRadius: Theme.Radius.panel))
        }
        .buttonStyle(.plain)
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
