import SwiftUI

// These pairs follow apps/desktop/src/theme.rs. Text uses iOS text styles rather than the
// desktop's macOS point sizes, so Dynamic Type scales it like the rest of the system.
enum Theme {
    static let background = color(0xFFFFFF, 0x1E1E1E)
    static let subtle = color(0xF5F5F3, 0x262626)
    static let raised = color(0xFFFFFF, 0x2C2C2C)
    static let ink = color(0x111110, 0xF2F2EE)
    static let secondary = color(0x5F5F5A, 0x9B9B95)
    static let line = color(0xE6E6E2, 0x3A3A3A)
    /// `sel_focus`: the accent as a fill under white text, the same in light and dark.
    static let selection = color(0x2A5BB8, 0x2A5BB8)
    static let accent = color(0x2A5BB8, 0x8AB0F5)
    static let done = color(0x1E6B4E, 0x4CC38A)
    static let later = color(0x94640A, 0xF5C22E)
    static let danger = color(0xC62828, 0xF87171)
    /// The current-time line in the agenda, as on the desktop week view.
    static let now = later

    /// Corner radii, `theme::radius`.
    enum Radius {
        static let key: CGFloat = 4
        static let control: CGFloat = 8
        static let attachment: CGFloat = 10
        static let panel: CGFloat = 12
        static let card: CGFloat = 16
    }

    /// Account marks cycle through the three accents (Dusk, pine green, gold), as `account_accent` does.
    static func account(_ index: Int) -> Color { [accent, done, later][max(index, 0) % 3] }

    private static func color(_ light: UInt32, _ dark: UInt32) -> Color {
        Color(uiColor: UIColor { traits in
            let hex = traits.userInterfaceStyle == .dark ? dark : light
            return UIColor(red: CGFloat((hex >> 16) & 255) / 255,
                           green: CGFloat((hex >> 8) & 255) / 255,
                           blue: CGFloat(hex & 255) / 255, alpha: 1)
        })
    }
}

/// An account's mark, only when several accounts share a view.
func accountAccent(_ accounts: [MailAccount], _ id: String) -> Color? {
    guard accounts.count > 1, let index = accounts.firstIndex(where: { $0.id == id }) else { return nil }
    return Theme.account(index)
}

struct RowPresentation: Equatable {
    let sender: String
    let subject: String
    let preview: String
    let date: Date
    let unread: Bool
    let hasAttachments: Bool

    init(_ row: MailRow) {
        sender = row.senderName.isEmpty ? row.senderAddress : row.senderName
        subject = row.subject.isEmpty ? "(No subject)" : row.subject
        preview = row.preview
        date = Date(timeIntervalSince1970: TimeInterval(row.dateUnix))
        unread = row.unread
        hasAttachments = row.hasAttachments
    }
}

struct MailRowView: View {
    let row: MailRow
    /// The account's mark; nil with a single account, as on desktop.
    var accent: Color? = nil
    var words = DateWords()

    var body: some View {
        let value = RowPresentation(row)
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Circle().fill(value.unread ? Theme.accent : .clear)
                .frame(width: 8, height: 8)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    if let accent {
                        RoundedRectangle(cornerRadius: 1).fill(accent).frame(width: 3, height: 14)
                            .alignmentGuide(.firstTextBaseline) { $0[.bottom] - 2 }
                            .accessibilityHidden(true)
                    }
                    Text(value.sender).font(.body.weight(value.unread ? .semibold : .regular)).lineLimit(1)
                    Spacer(minLength: 4)
                    if value.hasAttachments {
                        Image(systemName: "paperclip").font(.footnote).foregroundStyle(Theme.secondary).accessibilityLabel("Attachments")
                    }
                    Text(words.listTime(value.date)).font(.subheadline).foregroundStyle(Theme.secondary)
                }
                Text(value.subject).font(.subheadline).lineLimit(1)
                Text(value.preview).font(.subheadline).foregroundStyle(Theme.secondary).lineLimit(1)
            }
        }
        .foregroundStyle(Theme.ink)
        .padding(.vertical, 4)
        .accessibilityElement(children: .combine)
        .accessibilityValue(value.unread ? "Unread" : "Read")
    }
}
