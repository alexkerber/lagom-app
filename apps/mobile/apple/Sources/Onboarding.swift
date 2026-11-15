import SwiftUI

/// The intro before the app: Welcome, Choose account, then Notifications once an account is in.
/// Copy matches the Android intro word for word.
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
        .sheet(isPresented: $other) { AddAccountView(model: model, providers: false) }
    }

    /// One step: Back on Choose account, then the content centred and scrolling under large text.
    private func page(_ step: Step) -> some View {
        VStack(spacing: 0) {
            HStack {
                if step == .choose {
                    IconButton(title: "Back", icon: "chevron-left") { choosing = false; error = nil }.disabled(connecting)
                }
            }
            .padding(.horizontal, 10).frame(maxWidth: .infinity, minHeight: 56, alignment: .leading)
            ScrollView {
                VStack(spacing: 16) { content(step) }
                    .multilineTextAlignment(.center)
                    .padding(.horizontal, 24).padding(.bottom, 56)
                    .frame(maxWidth: 420)
                    .frame(maxWidth: .infinity, minHeight: height)
            }
            .scrollBounceBehavior(.basedOnSize)
            .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { height = $0 }
        }
        .background(Theme.background)
        .transition(.asymmetric(insertion: Motion.slide(step == .welcome ? .leading : .trailing, reduceMotion), removal: .opacity))
    }

    @ViewBuilder private func content(_ step: Step) -> some View {
        switch step {
        case .welcome:
            Image("Brand").resizable().frame(width: 72, height: 72).accessibilityHidden(true)
            Text("lagom").lagomFont(.subject, .bold).accessibilityAddTraits(.isHeader)
            Text("Mail, calendar and todo. Just right.").lagomFont(.body).foregroundStyle(Theme.secondary)
            HouseButton(title: "Add account", icon: "account-add", primary: true) { choosing = true }
                .padding(.top, 8)
        case .choose:
            Text("Choose your mail").lagomFont(.subject, .bold).accessibilityAddTraits(.isHeader)
            Text("Google and Microsoft sign in in the browser. Anything else uses its mail server.").lagomFont(.body).foregroundStyle(Theme.secondary)
            VStack(spacing: 8) {
                // Hidden until this build has the provider's client ID, as in Add account.
                ForEach([(SignInProvider.google, "Google"), (.microsoft, "Microsoft")], id: \.1) { provider, name in
                    if signInAvailable(provider: provider) {
                        choice(name) { Task { await signIn(provider) } }
                    }
                }
                choice("Other (IMAP)") { other = true }
            }
            .disabled(connecting)
            if connecting {
                HStack(spacing: 8) {
                    ProgressView()
                    Text("Signing in…").lagomFont(.small).foregroundStyle(Theme.secondary)
                }
                .accessibilityElement(children: .combine)
            }
            if let error { Text(error).lagomFont(.snippet).foregroundStyle(Theme.danger) }
        case .notifications:
            Text("New mail, when it arrives").lagomFont(.subject, .bold).accessibilityAddTraits(.isHeader)
            Text("lagom tells you about new mail in your Inbox. Nothing else.").lagomFont(.body).foregroundStyle(Theme.secondary)
            VStack(spacing: 8) {
                HouseButton(title: "Allow", icon: "done", primary: true, action: allow)
                HouseButton(title: "Not now", icon: "close", action: notNow)
            }
            .padding(.top, 8)
        }
    }

    private func choice(_ title: String, action: @escaping () -> Void) -> some View {
        Button(action: action) { Text(title).frame(maxWidth: .infinity) }.buttonStyle(HouseButtonStyle())
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
