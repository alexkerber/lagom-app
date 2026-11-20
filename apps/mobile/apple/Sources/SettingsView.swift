import SwiftUI
import UIKit

/// The Mac's Settings page (Alex 2026-11-20: one Settings page on every platform): Accounts, a row per account
/// that opens its page (Your name, Signature, Remove account), then General and About. A page in the iPad
/// sidebar; on a phone a sheet from the mailbox menu's last item. Mobile has no keyboard shortcuts.
struct SettingsView: View {
    let model: MailModel
    /// A phone's sheet, with Close.
    var sheet = false
    /// The account whose page is open; nil shows the list.
    @State private var open: String?
    @State private var addAccount = false
    @State private var removing: MailAccount?
    @State private var signing: MailAccount?
    @State private var naming: MailAccount?
    @State private var newName = ""
    @Environment(\.horizontalSizeClass) private var width
    /// Dynamic Type scale of the display size, so tracking stays the same share of an em.
    @ScaledMetric(relativeTo: .largeTitle) private var typeScale: CGFloat = 1
    @Environment(\.openURL) private var openURL
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 32) {
                if let index = model.accounts.firstIndex(where: { $0.id == open }) {
                    accountPage(model.accounts[index], color: Theme.account(index))
                } else {
                    mainPage
                }
            }
            // The Mac's 600 pt column, centred on iPad.
            .padding(.horizontal, width == .regular ? 24 : 16).padding(.vertical, 16)
            .frame(maxWidth: 600, alignment: .leading)
            .frame(maxWidth: .infinity)
        }
        .background(Theme.background)
        .foregroundStyle(Theme.ink)
        // A removed account's page closes.
        .onChange(of: model.accounts.map(\.id)) { _, ids in if let id = open, !ids.contains(id) { open = nil } }
        .onChange(of: model.accounts.first?.id, initial: true) { _, id in if Storage.settingsScreen == "account" && open == nil { open = id } }
        .sheet(isPresented: $addAccount) { AddAccountView(model: model).duoRail() }
        .sheet(item: $signing) { account in SignatureView(model: model, account: account).duoRail() }
        .alert("Your name", isPresented: Binding(get: { naming != nil }, set: { if !$0 { naming = nil } })) {
            TextField("Your name", text: $newName).textContentType(.name)
            Button("Save") {
                guard let account = naming else { return }
                Task { await model.setName(newName, account: account) }
            }
            Button("Cancel", role: .cancel) {}
        } message: { Text("Shown to the people you write to, and in the sidebar.") }
        // The Mac's words: the question names the account, Keep is the way out.
        .confirmationDialog(removing.map { "Remove \($0.email) from lagom?" } ?? "", isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }),
                            titleVisibility: .visible, presenting: removing) { account in
            Button("Remove", role: .destructive) { Task { await model.remove(account) }; removing = nil }
            Button("Keep", role: .cancel) { removing = nil }
        } message: { _ in Text("Your mail stays with your provider.") }
    }

    @ViewBuilder private var mainPage: some View {
        HStack(alignment: .firstTextBaseline) {
            Text("Settings").font(.custom(Theme.Face.display, size: width == .regular ? 48 : 34, relativeTo: .largeTitle))
                .tracking((width == .regular ? 48 : 34) * typeScale * Theme.Face.displayTracking)
                .accessibilityAddTraits(.isHeader)
            Spacer()
            closeButton
        }
        group("Accounts", note: "The email addresses in lagom. Only you see them.") {
            ForEach(Array(model.accounts.enumerated()), id: \.element.id) { index, account in
                if index > 0 { hairline(inset: 64) }
                row(title: account.email, detail: accountKind(account), mark: Avatar(account: account, color: Theme.account(index), side: 32)) {
                    open = account.id
                }
            }
            if !model.accounts.isEmpty { hairline(inset: 64) }
            row(title: "Add account", mark: Icon("account-add").foregroundStyle(Theme.secondary), chevron: false) { addAccount = true }
                .disabled(model.core == nil)
        }
        group("General") {
            // The system owns the switch on a phone; the row opens it.
            row(title: "Notifications for new mail", icon: "external") {
                if let url = URL(string: UIApplication.openNotificationSettingsURLString) { openURL(url) }
            }
        }
        group("About") {
            ForEach(Array(Self.about.enumerated()), id: \.offset) { index, item in
                if index > 0 { hairline(inset: 16) }
                row(title: item.title, value: item.value, icon: "external") { openURL(item.url) }
            }
        } footer: {
            Text("lagom \(Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "")")
                .lagomFont(.small).foregroundStyle(Theme.secondary).padding(.top, 8)
        }
    }

    /// The Mac's About rows, in its order.
    private static let about: [(title: String, value: String?, url: URL)] = [
        ("Source code", nil, URL(string: "https://github.com/alexkerber/lagom-app")!),
        ("Licence", "MIT", URL(string: "https://github.com/alexkerber/lagom-app/blob/main/LICENSE")!),
        ("Privacy", nil, URL(string: "https://lagommail.app/privacy.html")!),
        ("Support", nil, URL(string: "https://lagommail.app/support.html")!),
    ]

    /// One account, as on the Mac: Your name and Signature, then Remove account.
    @ViewBuilder private func accountPage(_ account: MailAccount, color: Color) -> some View {
        HStack {
            Button { open = nil } label: {
                HStack(spacing: 4) {
                    Icon("chevron-left")
                    Text("Settings").lagomFont(.base)
                }
                .foregroundStyle(Theme.secondary).frame(minHeight: 44).contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Back to Settings")
            Spacer()
            closeButton
        }
        .padding(.bottom, -16)
        HStack(spacing: 16) {
            Avatar(account: account, color: color, side: 48)
            VStack(alignment: .leading, spacing: 2) {
                Text(account.email).font(.custom(Theme.Face.display, size: TextSize.title.points, relativeTo: .title2))
                    .lineLimit(1).truncationMode(.middle).accessibilityAddTraits(.isHeader)
                if !account.kind.isEmpty { Text(account.kind).lagomFont(.small).foregroundStyle(Theme.secondary) }
            }
        }
        framed {
            let named = account.displayName != account.email
            row(title: "Your name", detail: named ? account.displayName : "Shown to the people you write to.") {
                newName = named ? account.displayName : ""
                naming = account
            }
            hairline(inset: 16)
            row(title: "Signature", detail: "Added to new mail, replies and forwards.") { signing = account }
        }
        group(nil, note: nil) {
            row(title: "Remove account", mark: Icon("remove-account").foregroundStyle(Theme.danger), chevron: false, tint: Theme.danger) { removing = account }
        } footer: {
            Text("Removes the account and its saved mail from this \(UIDevice.current.model). Your mail stays with your provider.")
                .lagomFont(.small).foregroundStyle(Theme.secondary).padding(.top, 8)
        }
    }

    @ViewBuilder private var closeButton: some View {
        if sheet { IconButton(title: "Close", icon: "close") { dismiss() }.padding(.trailing, -10) }
    }

    /// A titled group: a mono label, an optional line, then its rows in one hairline frame.
    private func group(_ title: String?, note: String? = nil, @ViewBuilder content: () -> some View,
                       @ViewBuilder footer: () -> some View = { EmptyView() }) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            if let title { MonoLabel(text: title).padding(.bottom, note == nil ? 8 : 4) }
            if let note { Text(note).lagomFont(.small).foregroundStyle(Theme.secondary).padding(.bottom, 12) }
            framed(content)
            footer()
        }
    }

    /// Rows in one hairline frame, like the intro's "Choose your mail" list.
    private func framed(@ViewBuilder _ content: () -> some View) -> some View {
        VStack(spacing: 0) { content() }
            .overlay { RoundedRectangle(cornerRadius: Theme.Radius.control).strokeBorder(Theme.line) }
            .clipShape(.rect(cornerRadius: Theme.Radius.control))
    }

    private func hairline(inset: CGFloat) -> some View { Rectangle().fill(Theme.line).frame(height: 1).padding(.leading, inset) }

    /// One row: an optional mark, the title over an optional quiet line, a value, then a chevron or an icon.
    private func row<Mark: View>(title: String, detail: String? = nil, value: String? = nil, mark: Mark? = Optional<EmptyView>.none,
                     chevron: Bool = true, icon: String? = nil, tint: Color = Theme.ink, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 16) {
                if let mark { mark.frame(width: 32).accessibilityHidden(true) }
                VStack(alignment: .leading, spacing: 2) {
                    Text(title).lagomFont(.body, .medium).foregroundStyle(tint).lineLimit(1).truncationMode(.middle)
                    if let detail { Text(detail).lagomFont(.small).foregroundStyle(Theme.secondary).lineLimit(2) }
                }
                Spacer(minLength: 8)
                if let value { Text(value).lagomFont(.base).foregroundStyle(Theme.secondary) }
                if let icon { Icon(icon, size: 14).foregroundStyle(Theme.secondary) }
                else if chevron { Icon("chevron-right", size: 14).foregroundStyle(Theme.secondary) }
            }
            .padding(.horizontal, 16).padding(.vertical, 8).frame(maxWidth: .infinity, minHeight: mark == nil && detail == nil ? 48 : 56)
            .contentShape(Rectangle())
        }
        .buttonStyle(RowStyle())
    }
}

/// The Mac's account mark: the initial in the account's colour on a tint of it.
private struct Avatar: View {
    let account: MailAccount
    let color: Color
    let side: CGFloat

    var body: some View {
        let name = account.displayName.trimmingCharacters(in: .whitespaces).isEmpty ? account.email : account.displayName
        Text(name.first { $0.isLetter || $0.isNumber }.map { String($0).uppercased() } ?? "")
            .font(.custom(Theme.Face.display, size: side * 0.42, relativeTo: .body))
            .foregroundStyle(color)
            .frame(width: side, height: side)
            .background(color.opacity(0.18), in: .circle)
            .accessibilityHidden(true)
    }
}
