import SwiftUI
import UIKit

/// The desktop's Settings page: accounts, notifications and about (Alex 2026-11-17). A page in the iPad
/// sidebar; on a phone a sheet from the mailbox menu, where accounts already live.
struct SettingsView: View {
    let model: MailModel
    var accounts = true
    @Binding var addAccount: Bool
    @Binding var removing: MailAccount?
    @Environment(\.horizontalSizeClass) private var width
    @Environment(\.openURL) private var openURL
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 32) {
                HStack(alignment: .firstTextBaseline) {
                    Text("Settings").font(.custom("Hauora-Bold", size: width == .regular ? 48 : 34, relativeTo: .largeTitle))
                        .accessibilityAddTraits(.isHeader)
                    Spacer()
                    if !accounts { IconButton(title: "Close", icon: "close") { dismiss() } }
                }
                if accounts { group("Accounts") {
                    ForEach(model.accounts, id: \.id) { account in
                        HStack {
                            Text(account.email).lagomFont(.base).lineLimit(1)
                            Spacer(minLength: 8)
                            Button("Remove") { removing = account }.lagomFont(.base).foregroundStyle(Theme.secondary)
                                .accessibilityLabel("Remove \(account.email)")
                        }
                        .frame(minHeight: 44)
                    }
                    row("Add account", icon: "plus") { addAccount = true }
                } }
                group("Notifications") {
                    row("Notification settings", icon: nil) {
                        if let url = URL(string: UIApplication.openNotificationSettingsURLString) { openURL(url) }
                    }
                }
                group("About") {
                    row("Privacy", icon: nil) { openURL(URL(string: "https://lagommail.app/privacy.html")!) }
                    row("Support", icon: nil) { openURL(URL(string: "https://lagommail.app/support.html")!) }
                    Text("lagom \(Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "")")
                        .lagomFont(.small).foregroundStyle(Theme.secondary).frame(minHeight: 44)
                }
            }
            .padding(.horizontal, width == .regular ? 24 : 16).padding(.vertical, 16)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(Theme.background)
        .foregroundStyle(Theme.ink)
    }

    private func group(_ title: String, @ViewBuilder content: () -> some View) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            MonoLabel(text: title).padding(.bottom, 8)
            content()
        }
    }

    private func row(_ title: String, icon: String?, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 10) {
                if let icon { Icon(icon) }
                Text(title).lagomFont(.base)
                Spacer(minLength: 0)
            }
            .frame(minHeight: 44).contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}
