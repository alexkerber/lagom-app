import SwiftUI

// House components after the desktop's (`main_window.rs`: `button`, `icon_button`, `sidebar_nav`,
// the list header and its All/Unread filter). Touch targets stay 44 pt; the drawn shapes keep the
// desktop's proportions.

/// A push button: bordered, or filled with the accent when `primary`. Desktop `button`.
struct HouseButtonStyle: ButtonStyle {
    var primary = false
    @Environment(\.isEnabled) private var enabled

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .lagomFont(.base, .medium)
            .foregroundStyle(primary ? Theme.accentInk : Theme.ink)
            .padding(.horizontal, 12)
            .frame(minHeight: 34)
            .background(primary ? Theme.accent : Theme.background, in: RoundedRectangle(cornerRadius: Theme.Radius.control))
            .overlay {
                if !primary { RoundedRectangle(cornerRadius: Theme.Radius.control).strokeBorder(Theme.line) }
            }
            .opacity(configuration.isPressed ? 0.8 : enabled ? 1 : 0.5)
            .frame(minHeight: 44)
            .contentShape(Rectangle())
    }
}

/// A borderless glyph, desktop `icon_button`: 32 pt drawn, 44 pt to touch.
struct IconButtonStyle: ButtonStyle {
    @Environment(\.isEnabled) private var enabled

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .foregroundStyle(Theme.secondary)
            .frame(minWidth: 32, minHeight: 32)
            .background(configuration.isPressed ? Theme.ink.opacity(0.12) : .clear, in: RoundedRectangle(cornerRadius: Theme.Radius.control))
            .opacity(enabled ? 1 : 0.4)
            .frame(minWidth: 44, minHeight: 44)
            .contentShape(Rectangle())
    }
}

/// An icon and a label, as every desktop action button draws them. `iconOnly` keeps the label for VoiceOver.
struct ButtonLabel: View {
    let title: String
    let icon: String
    var primary = false
    var iconOnly = false

    var body: some View {
        HStack(spacing: 6) {
            Icon(icon).foregroundStyle(primary ? Theme.accentInk : Theme.secondary)
            if !iconOnly { Text(title).lineLimit(1) }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(title)
    }
}

struct HouseButton: View {
    let title: String
    let icon: String
    var primary = false
    var iconOnly = false
    let action: () -> Void

    var body: some View {
        Button(action: action) { ButtonLabel(title: title, icon: icon, primary: primary, iconOnly: iconOnly) }
            .buttonStyle(HouseButtonStyle(primary: primary))
    }
}

struct IconButton: View {
    let title: String
    let icon: String
    let action: () -> Void

    var body: some View {
        Button(action: action) { Icon(icon) }
            .buttonStyle(IconButtonStyle())
            .accessibilityLabel(title)
    }
}

/// The filled count on Mail in the sidebar and the bar. Numbers roll, they do not jump.
struct Badge: View {
    let count: Int
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        Text("\(count)").lagomFont(.small, .semibold).monospacedDigit()
            .contentTransition(.numericText(value: Double(count)))
            .animation(reduceMotion ? nil : Motion.fade, value: count)
            .foregroundStyle(Theme.accentInk)
            .padding(.horizontal, 5).frame(minWidth: 18, minHeight: 18)
            .background(Theme.accent, in: Capsule())
    }
}

/// A spinner that appears only once a wait passes `Motion.calm`, so a quick load never flashes it.
struct CalmProgress: View {
    let label: String
    @State private var shown = false

    var body: some View {
        ProgressView().accessibilityLabel(label)
            .opacity(shown ? 1 : 0)
            .task {
                guard (try? await Task.sleep(for: Motion.calm)) != nil else { return }
                withAnimation(Motion.fade) { shown = true }
            }
    }
}

/// A list's date group, "Today", "Yesterday", "Earlier".
struct GroupHeader: View {
    let title: String

    var body: some View {
        Text(title).lagomFont(.small).foregroundStyle(Theme.secondary)
            .padding(.horizontal, 10).frame(minHeight: 30, alignment: .bottom).padding(.bottom, 4)
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityAddTraits(.isHeader)
    }
}

/// A page title as on desktop: semibold at the title size, actions on the right.
struct PageTitle<Trailing: View>: View {
    let title: String
    @ViewBuilder var trailing: Trailing

    var body: some View {
        HStack(spacing: 8) {
            Text(title).lagomFont(.title, .semibold).accessibilityAddTraits(.isHeader)
            Spacer(minLength: 8)
            trailing
        }
        .foregroundStyle(Theme.ink)
        .frame(minHeight: 44)
    }
}

/// A section heading in a page: Schedule, To do, a task list's name.
struct Heading: View {
    let title: String

    var body: some View {
        Text(title).lagomFont(.base, .semibold).foregroundStyle(Theme.ink).accessibilityAddTraits(.isHeader)
    }
}

/// The desktop's empty pages: a large line icon, or the desktop's line illustration, over a quiet
/// line, and an optional action.
struct EmptyState<Action: View>: View {
    let icon: String
    let title: String
    var detail: String? = nil
    var illustration: String? = nil
    @ViewBuilder var action: Action

    var body: some View {
        VStack(spacing: 10) {
            if let illustration {
                Image(illustration).resizable().scaledToFit().frame(maxWidth: 240)
                    .foregroundStyle(Theme.ink).padding(.bottom, 12).accessibilityHidden(true)
            } else {
                Icon(icon, size: 36).foregroundStyle(Theme.secondary)
            }
            Text(title).lagomFont(.base, .semibold).foregroundStyle(Theme.ink)
            if let detail {
                Text(detail).lagomFont(.snippet).foregroundStyle(Theme.secondary)
            }
            action.padding(.top, 4)
        }
        .multilineTextAlignment(.center)
        .padding(32)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

extension EmptyState where Action == EmptyView {
    init(icon: String, title: String, detail: String? = nil, illustration: String? = nil) {
        self.init(icon: icon, title: title, detail: detail, illustration: illustration) { EmptyView() }
    }
}

/// All / Unread, the desktop list header's filter: icons and labels in a small segmented well.
struct FilterControl: View {
    @Binding var unreadOnly: Bool
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        HStack(spacing: 0) {
            segment("All", icon: "filter-all", unread: false)
            segment("Unread", icon: "unread", unread: true)
        }
        // The highlight moves in 140 ms ease-out, as the desktop selection; at once under Reduce Motion.
        .animation(reduceMotion ? nil : .easeOut(duration: 0.14), value: unreadOnly)
        .padding(2)
        .background(Theme.subtle, in: RoundedRectangle(cornerRadius: Theme.Radius.control))
        .overlay(RoundedRectangle(cornerRadius: Theme.Radius.control).strokeBorder(Theme.line))
    }

    private func segment(_ title: String, icon: String, unread: Bool) -> some View {
        let on = unreadOnly == unread
        return Button { unreadOnly = unread } label: {
            HStack(spacing: 6) {
                Icon(icon, size: 16).foregroundStyle(on ? Theme.ink : Theme.secondary)
                Text(title).lagomFont(.small).foregroundStyle(Theme.ink)
            }
            .padding(.horizontal, 10).frame(minHeight: 28)
            .background(on ? Theme.raised : .clear, in: RoundedRectangle(cornerRadius: 6))
            .shadow(color: on ? .black.opacity(0.06) : .clear, radius: 1, y: 0.5)
            // A 44 pt hit target around the drawn 28 pt segment.
            .padding(.vertical, 8).contentShape(Rectangle()).padding(.vertical, -8)
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(on ? .isSelected : [])
    }
}

/// The desktop sidebar's Search field.
struct SearchField: View {
    @Binding var text: String

    var body: some View {
        HStack(spacing: 6) {
            Icon("search").foregroundStyle(Theme.secondary)
            TextField("Search", text: $text)
                .lagomFont(.base).foregroundStyle(Theme.ink)
                .textInputAutocapitalization(.never).autocorrectionDisabled().submitLabel(.search)
            if !text.isEmpty {
                Button { text = "" } label: { Icon("close", size: 16) }
                    .buttonStyle(IconButtonStyle()).accessibilityLabel("Clear search")
            }
        }
        .padding(.horizontal, 8).frame(minHeight: 44)
        .background(Theme.raised, in: RoundedRectangle(cornerRadius: Theme.Radius.control))
        .overlay(RoundedRectangle(cornerRadius: Theme.Radius.control).strokeBorder(Theme.line))
    }
}

/// A sheet as the desktop compose sheet: a subtle title bar with Cancel, the content, and a footer
/// whose first item is the primary action.
struct SheetScaffold<Content: View, Footer: View>: View {
    let title: String
    var cancelDisabled = false
    let cancel: () -> Void
    @ViewBuilder var content: Content
    @ViewBuilder var footer: Footer

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text(title).lagomFont(.base, .semibold).accessibilityAddTraits(.isHeader)
                Spacer()
                IconButton(title: "Cancel", icon: "close", action: cancel).disabled(cancelDisabled)
            }
            .padding(.leading, 16).padding(.trailing, 6).frame(minHeight: 50)
            .background(Theme.subtle)
            .overlay(alignment: .bottom) { Theme.line.frame(height: 1) }
            content
            HStack(spacing: 10) { footer }
                .padding(.horizontal, 16).frame(minHeight: 64)
                .overlay(alignment: .top) { Theme.line.frame(height: 1) }
        }
        .foregroundStyle(Theme.ink)
        .background(Theme.raised)
    }
}

/// One labelled line in a sheet, desktop compose `field`: a quiet label, the control, a hairline under.
struct FieldRow<Content: View>: View {
    let label: String
    @ViewBuilder var content: Content
    // Grows with the label's text style, so "Subject" never clips at large Dynamic Type sizes.
    @ScaledMetric(relativeTo: .footnote) private var labelWidth: CGFloat = 64

    var body: some View {
        HStack(spacing: 16) {
            Text(label).lagomFont(.small).foregroundStyle(Theme.secondary).frame(width: labelWidth, alignment: .leading)
                .accessibilityHidden(true)
            content.lagomFont(.base).frame(maxWidth: .infinity, alignment: .leading)
        }
        .frame(minHeight: 50)
        .overlay(alignment: .bottom) { Theme.line.frame(height: 1) }
    }
}

/// The phone's page bar: the desktop sidebar's four pages with their icons and the Mail badge.
struct LagomBar: View {
    @Binding var section: AppSection
    let unread: Int

    var body: some View {
        HStack(spacing: 4) {
            ForEach(AppSection.allCases) { page in
                let active = page == section
                Button { section = page } label: {
                    VStack(spacing: 3) {
                        Icon(page.icon, size: 20).foregroundStyle(active ? Theme.selectionIcon : Theme.secondary)
                            .overlay(alignment: .topTrailing) {
                                if page == .mail && unread > 0 { Badge(count: unread).fixedSize().offset(x: 14, y: -6) }
                            }
                        Text(page.title).lagomFont(.small, active ? .medium : .regular).foregroundStyle(Theme.ink)
                    }
                    .frame(maxWidth: .infinity, minHeight: 50)
                    .background(active ? Theme.selected : .clear, in: RoundedRectangle(cornerRadius: Theme.Radius.control))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityElement(children: .ignore)
                .accessibilityLabel(page == .mail && unread > 0 ? "Mail, \(unread) unread" : page.title)
                .accessibilityAddTraits(active ? [.isSelected, .isButton] : .isButton)
                // The bar stops growing at xxLarge; a long press shows the item large.
                .accessibilityShowsLargeContentViewer {
                    Icon(page.icon)
                    Text(page.title)
                }
            }
        }
        .dynamicTypeSize(...DynamicTypeSize.xxLarge)
        .padding(.horizontal, 10).padding(.top, 6).padding(.bottom, 2)
        .background(Theme.sidebar.ignoresSafeArea(edges: .bottom))
        .overlay(alignment: .top) { Theme.line.frame(height: 1) }
    }
}

/// One sidebar line. Pages fill grey when active; mailboxes turn accent, as `sidebar_nav` and `mailbox_style`.
struct SidebarRow<Leading: View>: View {
    let title: String
    var count: Int = 0
    var badge = false
    var active = false
    var page = false
    @ViewBuilder var leading: Leading
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 8) {
                leading
                Text(title).lagomFont(.base, active && !page ? .medium : .regular).lineLimit(1)
                    .foregroundStyle(active && !page ? Theme.accent : Theme.ink)
                Spacer(minLength: 4)
                if count > 0 {
                    if badge { Badge(count: count) } else {
                        Text("\(count)").lagomFont(.small).monospacedDigit().foregroundStyle(Theme.secondary)
                    }
                }
            }
            .padding(.horizontal, 10).frame(minHeight: 44)
            .background(active && page ? Theme.selected : .clear, in: RoundedRectangle(cornerRadius: Theme.Radius.control))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(active ? .isSelected : [])
    }
}

extension SidebarRow where Leading == AnyView {
    /// A row led by a desktop icon: `selectionIcon` for the active page, accent for the open mailbox.
    init(_ title: String, icon: String, count: Int = 0, badge: Bool = false, active: Bool = false, page: Bool = false, action: @escaping () -> Void) {
        self.init(title: title, count: count, badge: badge, active: active, page: page, leading: {
            AnyView(Icon(icon).foregroundStyle(active ? (page ? Theme.selectionIcon : Theme.accent) : Theme.secondary))
        }, action: action)
    }
}
