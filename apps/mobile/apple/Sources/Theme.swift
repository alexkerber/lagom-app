import SwiftUI

// These pairs follow apps/desktop/src/theme.rs; Dynamic Type scales its SF Pro sizes.
enum Theme {
    static let background = color(0xFFFFFF, 0x1E1E1E)
    static let subtle = color(0xF5F5F3, 0x262626)
    static let raised = color(0xFFFFFF, 0x2C2C2C)
    static let ink = color(0x111110, 0xF2F2EE)
    static let secondary = color(0x5F5F5A, 0x9B9B95)
    static let accent = color(0x2A5BB8, 0x8AB0F5)
    static let done = color(0x1E6B4E, 0x4CC38A)
    static let later = color(0x94640A, 0xF5C22E)
    static let danger = color(0xC62828, 0xF87171)

    private static func color(_ light: UInt32, _ dark: UInt32) -> Color {
        Color(uiColor: UIColor { traits in
            let hex = traits.userInterfaceStyle == .dark ? dark : light
            return UIColor(red: CGFloat((hex >> 16) & 255) / 255,
                           green: CGFloat((hex >> 8) & 255) / 255,
                           blue: CGFloat(hex & 255) / 255, alpha: 1)
        })
    }
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
    @ScaledMetric(relativeTo: .body) private var base = 13.0
    @ScaledMetric(relativeTo: .caption) private var snippet = 12.0

    var body: some View {
        let value = RowPresentation(row)
        HStack(alignment: .top, spacing: 8) {
            Circle().fill(value.unread ? Theme.accent : .clear)
                .frame(width: 6, height: 6).padding(.top, 6)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 4) {
                HStack {
                    Text(value.sender).fontWeight(value.unread ? .semibold : .regular).lineLimit(1)
                    Spacer(minLength: 4)
                    Text(value.date, style: .time).font(.system(size: snippet)).foregroundStyle(Theme.secondary)
                    if value.hasAttachments { Image(systemName: "paperclip").accessibilityLabel("Attachments") }
                }
                Text(value.subject).lineLimit(1)
                Text(value.preview).font(.system(size: snippet)).foregroundStyle(Theme.secondary).lineLimit(1)
            }
        }
        .font(.system(size: base)).foregroundStyle(Theme.ink)
        .padding(.vertical, 4)
        .accessibilityElement(children: .combine)
        .accessibilityValue(value.unread ? "Unread" : "Read")
    }
}
