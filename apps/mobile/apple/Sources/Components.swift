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
            // No outline, as desktop since #168: a quiet fill marks the button.
            .background(primary ? Theme.accent : Theme.subtle, in: RoundedRectangle(cornerRadius: Theme.Radius.control))
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

/// The unread count on Mail in the sidebar: a quiet number, as on desktop (Alex 2026-11-15). Numbers roll, they do not jump.
struct Badge: View {
    let count: Int
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        Text("\(count)").lagomFont(.small).monospacedDigit()
            .contentTransition(.numericText(value: Double(count)))
            .animation(reduceMotion ? nil : Motion.fade, value: count)
            .foregroundStyle(Theme.secondary)
    }
}

/// Placeholder rows while the first sync runs, shaped like message rows and fading down the list (desktop
/// `loading_rows`, Android `LoadingRows`). They pulse slowly; under Reduce Motion they stay still.
struct LoadingRows: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var dim = false

    var body: some View {
        GeometryReader { geometry in
        let room = max(geometry.size.width - 49, 0)
        VStack(spacing: 0) {
            ForEach(Array([0.42, 0.3, 0.5, 0.36, 0.46].enumerated()), id: \.offset) { index, width in
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(Array([(width, 10.0), (width + 0.3, 10.0), (0.9, 8.0), (0.62, 8.0)].enumerated()), id: \.offset) { _, bar in
                        RoundedRectangle(cornerRadius: Theme.Radius.key).fill(Theme.line)
                            .frame(width: room * bar.0, height: bar.1)
                    }
                }
                .frame(maxWidth: .infinity, minHeight: 92, alignment: .leading)
                .padding(.leading, 33).padding(.trailing, 16)
                .opacity(1 - Double(index) * 0.18)
            }
        }
        }
        .opacity(dim ? 0.7 : 1)
        .onAppear {
            guard !reduceMotion else { return }
            withAnimation(.easeInOut(duration: 0.8).repeatForever(autoreverses: true)) { dim = true }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Loading mail")
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

/// The desktop's empty pages: a large line icon over a quiet
/// line, and an optional action.
struct EmptyState<Action: View>: View {
    let icon: String
    let title: String
    var detail: String? = nil
    @ViewBuilder var action: Action

    var body: some View {
        VStack(spacing: 10) {
            Icon(icon, size: 36).foregroundStyle(Theme.secondary)
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
    init(icon: String, title: String, detail: String? = nil) {
        self.init(icon: icon, title: title, detail: detail) { EmptyView() }
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
    }

    private func segment(_ title: String, icon: String, unread: Bool) -> some View {
        let on = unreadOnly == unread
        return Button { unreadOnly = unread } label: {
            HStack(spacing: 6) {
                Icon(icon, size: 16).foregroundStyle(on ? Theme.ink : Theme.secondary)
                // One line: "Unread" broke in two on the folded Duo.
                Text(title).lagomFont(.small).foregroundStyle(Theme.ink).lineLimit(1).fixedSize()
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
        .background(Theme.subtle, in: RoundedRectangle(cornerRadius: Theme.Radius.control))
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
            content
            HStack(spacing: 10) { footer }
                .padding(.horizontal, 16).frame(minHeight: 64)
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

/// A desktop icon at the tab bar's 24 pt; the bar draws an asset at its own 16 pt size.
func tabIcon(_ name: String) -> UIImage {
    guard let icon = UIImage(named: name) else { return UIImage() }
    let side = CGSize(width: 24, height: 24)
    return UIGraphicsImageRenderer(size: side).image { _ in icon.draw(in: CGRect(origin: .zero, size: side)) }
        .withRenderingMode(.alwaysTemplate)
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

/// The desktop swipe on a mail row: the row follows the finger, the colour behind it shows the move a release
/// makes (`SwipeStage`), a light tick marks each stage, and a release short of a stage springs back.
struct StageSwipe: ViewModifier {
    let canDelete: Bool
    let enabled: Bool
    let act: (SwipeStage) -> Void
    @State private var dx: CGFloat = 0
    @State private var width: CGFloat = 1
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func body(content: Content) -> some View {
        let stage = SwipeStage(dx: dx, width: width, canDelete: canDelete)
        content
            // Opaque, so the colour behind never shows through the text.
            .background(Theme.background)
            .offset(x: dx)
            .background { backdrop(stage) }
            .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { width = max($0, 1) }
            .gesture(HorizontalPan(enabled: enabled, changed: { dx = $0 }, ended: release))
            .sensoryFeedback(.impact(weight: .light), trigger: stage) { _, new in new != nil }
    }

    private func release(_ x: CGFloat) {
        guard let stage = SwipeStage(dx: x, width: width, canDelete: canDelete) else {
            withAnimation(Motion.move(reduceMotion)) { dx = 0 }
            return
        }
        // The row leaves the way it was going; the list drops it, and a refusal brings it back with the reload.
        withAnimation(Motion.move(reduceMotion)) { dx = x > 0 ? width : -width } completion: {
            act(stage)
            dx = 0
        }
    }

    private func backdrop(_ stage: SwipeStage?) -> some View {
        let right = dx > 0
        let shown = stage ?? (right ? .done : .later)
        let (fill, icon, title): (Color, String, String) = switch shown {
        case .done: (Theme.done, "done", "Done")
        case .later: (Theme.later, "later", "Later")
        case .delete: (Theme.danger, "trash", "Delete")
        }
        return HStack(spacing: 8) {
            Icon(icon, size: 20)
            Text(title).lagomFont(.base, .medium)
        }
        .foregroundStyle(stage == nil ? Theme.secondary : Theme.accentInk)
        .padding(.horizontal, 20)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: right ? .leading : .trailing)
        .background(stage == nil ? Theme.subtle : fill)
        .animation(.easeOut(duration: 0.1), value: stage)
        .opacity(dx == 0 ? 0 : 1)
        .accessibilityHidden(true)
    }
}

/// A pan that starts only when the finger moves more sideways than up or down, so the list still scrolls.
private struct HorizontalPan: UIGestureRecognizerRepresentable {
    let enabled: Bool
    let changed: (CGFloat) -> Void
    let ended: (CGFloat) -> Void

    func makeUIGestureRecognizer(context: Context) -> UIPanGestureRecognizer {
        let pan = UIPanGestureRecognizer()
        pan.delegate = context.coordinator
        return pan
    }

    func updateUIGestureRecognizer(_ pan: UIPanGestureRecognizer, context: Context) { pan.isEnabled = enabled }

    func makeCoordinator(converter: CoordinateSpaceConverter) -> Coordinator { Coordinator() }

    func handleUIGestureRecognizerAction(_ pan: UIPanGestureRecognizer, context: Context) {
        let x = pan.translation(in: pan.view).x
        switch pan.state {
        case .changed: changed(x)
        case .ended, .cancelled, .failed: ended(pan.state == .ended ? x : 0)
        default: break
        }
    }

    final class Coordinator: NSObject, UIGestureRecognizerDelegate {
        func gestureRecognizerShouldBegin(_ gesture: UIGestureRecognizer) -> Bool {
            guard let pan = gesture as? UIPanGestureRecognizer else { return false }
            let velocity = pan.velocity(in: pan.view)
            return abs(velocity.x) > abs(velocity.y)
        }
    }
}

/// The width of a right-edge column the system gives a phone, as on the iPhone Duo, where the status bar
/// and the tab bar sit at the right. Zero elsewhere. Mail puts its New message and filter buttons there.
private struct RailInsetKey: EnvironmentKey { static let defaultValue: CGFloat = 0 }
extension EnvironmentValues {
    var railInset: CGFloat {
        get { self[RailInsetKey.self] }
        set { self[RailInsetKey.self] = newValue }
    }
}
