import SwiftUI

// BEGIN GENERATED TOKENS (scripts/tokens.py)
extension Theme {
    static let background = color(0xFFFFFF, 0x1E1E1E)
    static let subtle = color(0xF5F5F3, 0x262626)
    static let raised = color(0xFFFFFF, 0x2C2C2C)
    static let ink = color(0x111110, 0xF2F2EE)
    static let secondary = color(0x5F5F5A, 0x9B9B95)
    static let line = color(0xE6E6E2, 0x3A3A3A)
    static let selected = color(0xDCDCDC, 0x464646)
    static let selection = color(0x2A5BB8, 0x2A5BB8)
    static let selectionIcon = color(0x2A5BB8, 0x94ADDC)
    static let accent = color(0x2A5BB8, 0x8AB0F5)
    static let accentInk = color(0xFFFFFF, 0x1E1E1E)
    static let accentFill = color(0x2A5BB8, 0x2F63CC)
    static let accentFillInk = color(0xFFFFFF, 0xFFFFFF)
    static let done = color(0x1E6B4E, 0x4CC38A)
    static let later = color(0x94640A, 0xF5C22E)
    static let danger = color(0xC62828, 0xF87171)
    static let sidebar = color(0xF3F2EF, 0x2A2A2A)

    enum Radius {
        static let key: CGFloat = 4
        static let control: CGFloat = 8
        static let attachment: CGFloat = 10
        static let panel: CGFloat = 12
        static let card: CGFloat = 16
    }

    /// The text family (SwiftUI picks the weight), PostScript names of the other faces; tracking in ems.
    enum Face {
        static let text = "Hauora"
        static let display = "Hauora-Bold"
        static let label = "GeistMono-Medium"
        static let displayTracking: CGFloat = -0.045
    }
}

extension TextSize {
    var points: CGFloat {
        switch self {
        case .small: 13
        case .snippet: 14
        case .base: 15
        case .body: 16
        case .title: 17
        case .subject: 23
        case .display: 30
        case .hero: 56
        }
    }
}

extension Motion {
    static let spring = Animation.spring(duration: 0.28, bounce: 0.15)
    static let fade = Animation.easeOut(duration: 0.15)
    static let calm = Duration.milliseconds(150)
    static let calmSeconds: Double = 0.15
}
// END GENERATED TOKENS

/// Shared tokens from design/tokens.toml, light and dark.
enum Theme {
    /// The current-time line in the agenda, as on the desktop week view.
    static let now = later

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

/// docs/motion.md on iOS: a spring where something moves and lands, ease-out for fades, and under
/// Reduce Motion nothing travels, it only fades.
enum Motion {
    /// The spring, or a fade under Reduce Motion.
    static func move(_ reduce: Bool) -> Animation { reduce ? fade : spring }
    /// Slides in from `edge` and fades, or only fades under Reduce Motion.
    static func slide(_ edge: Edge, _ reduce: Bool) -> AnyTransition {
        reduce ? .opacity : .move(edge: edge).combined(with: .opacity)
    }
    /// Fades in once `calm` has passed and leaves at once, so a state shown for a moment never flashes.
    static var calmIn: AnyTransition { .asymmetric(insertion: .opacity.animation(fade.delay(calmSeconds)), removal: .identity) }
}

/// The desktop type scale, `theme::size`, in Hauora as on desktop. A macOS point reads
/// smaller than an iOS point at phone distance, so every size is the desktop's × 1.15 (body 13 → 15),
/// which keeps the desktop's ratios. Each size scales with Dynamic Type from its text style.
enum TextSize {
    case small, snippet, base, body, title, subject, display, hero

    var style: Font.TextStyle {
        switch self {
        case .small: .footnote
        case .snippet: .subheadline
        case .base, .body: .body
        case .title: .headline
        case .subject: .title2
        case .display, .hero: .largeTitle
        }
    }
}

/// The text face; it scales with Dynamic Type from `style`. A custom face ignores the Bold Text
/// setting, so this steps the weight up itself, as the system font does.
struct LagomFont: ViewModifier {
    @Environment(\.legibilityWeight) private var legibility
    let points: CGFloat
    let style: Font.TextStyle
    var weight: Font.Weight = .regular

    func body(content: Content) -> some View {
        let bold = legibility == .bold
        let weight = !bold ? weight : weight == .regular ? .semibold : .bold
        content.font(.custom(Theme.Face.text, size: points, relativeTo: style).weight(weight))
    }
}

extension View {
    func lagomFont(_ size: TextSize, _ weight: Font.Weight = .regular) -> some View {
        modifier(LagomFont(points: size.points, style: size.style, weight: weight))
    }
}

/// A desktop line icon (`apps/desktop/assets/icons`), 16 pt there, tinted by the foreground style.
struct Icon: View {
    let name: String
    /// What VoiceOver reads; without one the icon is decoration.
    var label: String?
    @ScaledMetric private var side: CGFloat

    init(_ name: String, size: CGFloat = 18, label: String? = nil) {
        self.name = name
        self.label = label
        _side = ScaledMetric(wrappedValue: size, relativeTo: .body)
    }

    var body: some View {
        let image = Image(name).renderingMode(.template).resizable().frame(width: side, height: side)
        if let label { image.accessibilityLabel(label) } else { image.accessibilityHidden(true) }
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

/// The desktop list row: unread dot, account bar, sender and time; subject and a two-line snippet
/// under them. `selected` paints the accent fill with white text, as the focused desktop list does.
struct MailRowView: View {
    let row: MailRow
    /// The account's mark; nil with a single account, as on desktop.
    var accent: Color? = nil
    var selected = false
    var words = DateWords()

    var body: some View {
        let value = RowPresentation(row)
        let ink = selected ? Color.white : Theme.ink
        let ink2 = selected ? Color.white.opacity(0.85) : Theme.secondary
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 6) {
                Circle().fill(value.unread ? (selected ? Color.white : Theme.accent) : .clear)
                    .frame(width: 7, height: 7)
                if let accent {
                    RoundedRectangle(cornerRadius: 1.5).fill(accent).frame(width: 3, height: 16)
                }
                Text(value.sender).lagomFont(.base, value.unread ? .semibold : .regular).lineLimit(1)
                Spacer(minLength: 4)
                if row.messageCount > 1 {
                    Text(String(row.messageCount)).lagomFont(.small).monospacedDigit().foregroundStyle(Theme.ink)
                        .padding(.horizontal, 5).background(Theme.subtle, in: Capsule())
                        .accessibilityLabel("\(row.messageCount) messages")
                }
                if value.hasAttachments {
                    Icon("attachment", size: 15).foregroundStyle(ink2)
                }
                Text(words.listTime(value.date)).lagomFont(.small).monospacedDigit().foregroundStyle(ink2)
            }
            Group {
                Text(value.subject).lagomFont(.base).lineLimit(1)
                Text(value.preview).lagomFont(.snippet).foregroundStyle(ink2).lineLimit(2, reservesSpace: true)
            }
            .padding(.leading, 13)
        }
        .foregroundStyle(ink)
        .padding(.horizontal, 10).padding(.vertical, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(selected ? Theme.selection : .clear, in: RoundedRectangle(cornerRadius: Theme.Radius.control))
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
        .accessibilityValue([value.unread ? "Unread" : "Read", value.hasAttachments ? "Attachments" : nil].compactMap { $0 }.joined(separator: ", "))
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}
