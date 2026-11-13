import SwiftUI
import WebKit
import QuickLook

/// The desktop reading pane: the subject, then the message card (sender, date, actions menu, body,
/// attachments), then the reply box.
struct MessageView: View {
    let message: MailMessage
    @Bindable var model: MailModel
    @Environment(\.colorScheme) private var scheme
    @Environment(\.horizontalSizeClass) private var width
    @State private var showText = false

    var body: some View {
        let row = message.row
        let pad: CGFloat = width == .regular ? 24 : 16
        VStack(alignment: .leading, spacing: 16) {
            Text(RowPresentation(row).subject).lagomFont(.subject, .semibold).lineLimit(3).textSelection(.enabled)
                .accessibilityAddTraits(.isHeader)
            VStack(alignment: .leading, spacing: 20) {
                header
                if message.htmlBody != nil && !showText && model.remoteImagesID != row.id {
                    HStack(spacing: 10) {
                        HouseButton(title: "Load remote images", icon: "image") { model.loadRemoteImages(id: row.id) }
                        Text("Remote images are blocked").lagomFont(.small).foregroundStyle(Theme.secondary)
                    }
                }
                content
                if !message.attachments.isEmpty { attachments }
            }
            .padding(width == .regular ? 20 : 16)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .background(Theme.raised, in: RoundedRectangle(cornerRadius: Theme.Radius.card))
            .overlay(RoundedRectangle(cornerRadius: Theme.Radius.card).strokeBorder(Theme.line))
            Button { Task { await model.prepareCompose(id: row.id, kind: .reply) } } label: {
                HStack(spacing: 8) {
                    Icon("reply")
                    Text("Reply to \(RowPresentation(row).sender)").lagomFont(.base).lineLimit(1)
                    Spacer(minLength: 0)
                    if model.preparingCompose { ProgressView().accessibilityLabel("Preparing message") }
                }
                .foregroundStyle(Theme.secondary)
                .padding(.horizontal, 12).frame(minHeight: 44)
                .background(Theme.raised, in: RoundedRectangle(cornerRadius: Theme.Radius.control))
                .overlay(RoundedRectangle(cornerRadius: Theme.Radius.control).strokeBorder(Theme.line))
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(model.preparingCompose)
        }
        .padding(pad)
        .frame(maxWidth: 760 + 2 * pad, maxHeight: .infinity, alignment: .topLeading)
        .frame(maxWidth: .infinity, alignment: .leading)
        .foregroundStyle(Theme.ink)
        .quickLookPreview($model.attachmentURL)
        .onChange(of: message.row.id) { _, _ in showText = false }
    }

    /// Initials, sender, address and date, with the desktop's message menu.
    private var header: some View {
        let row = message.row
        let sender = RowPresentation(row).sender
        let initials = sender.split(separator: " ").prefix(2).compactMap(\.first).map(String.init).joined().uppercased()
        return HStack(spacing: 12) {
            Text(initials).lagomFont(.small, .semibold)
                .frame(width: 36, height: 36)
                .background(Theme.subtle, in: Circle())
                .overlay(Circle().strokeBorder(Theme.line))
                .accessibilityHidden(true)
            // The address wraps, and a very long one keeps its domain, so a look-alike sender stays visible.
            VStack(alignment: .leading, spacing: 2) {
                Text(sender).lagomFont(.base, .semibold).lineLimit(2)
                Text(row.senderAddress).lagomFont(.small).foregroundStyle(Theme.secondary)
                    .lineLimit(4).truncationMode(.middle).textSelection(.enabled)
                Text(DateWords().readingDate(Date(timeIntervalSince1970: TimeInterval(row.dateUnix))))
                    .lagomFont(.small).monospacedDigit().foregroundStyle(Theme.secondary)
            }
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
            Menu {
                Button("Reply all") { Task { await model.prepareCompose(id: row.id, kind: .replyAll) } }
                Button("Forward") { Task { await model.prepareCompose(id: row.id, kind: .forward) } }
                Button(row.unread ? "Mark read" : "Mark unread") { Task { await model.setRead(id: row.id, read: row.unread) } }
                if message.htmlBody != nil {
                    Button(showText ? "Show formatted" : "Show plain text") { showText.toggle() }
                }
            } label: { Icon("more") }
                .menuStyle(.button).buttonStyle(IconButtonStyle())
                .accessibilityLabel("Message actions")
                .padding(.trailing, -10)
        }
    }

    @ViewBuilder private var content: some View {
        if let html = message.htmlBody, !showText {
            if let core = model.core {
                MailHTML(document: mailDocument(html: html, dark: scheme == .dark, messageId: message.row.id,
                                                remote: model.remoteImagesID == message.row.id),
                         messageID: message.row.id, core: core)
                    .id("\(message.row.id)-\(model.inlineRevision)")
            }
        } else {
            ScrollView {
                Text(message.textBody.isEmpty ? message.row.preview : message.textBody)
                    .lagomFont(.body).lineSpacing(3)
                    .foregroundStyle(message.textBody.isEmpty ? Theme.secondary : Theme.ink)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
    }

    private var attachments: some View {
        ScrollView(.horizontal) {
            HStack(spacing: 8) {
                ForEach(message.attachments, id: \.section) { part in
                    let name = safeAttachmentName(part.filename)
                    let size = ByteCountFormatter.string(fromByteCount: Int64(part.size), countStyle: .file)
                    let downloading = model.downloadingAttachments.contains("\(message.row.id):\(part.section)")
                    Button { Task { await model.openAttachment(messageID: message.row.id, part: part) } } label: {
                        HStack(spacing: 12) {
                            if downloading { ProgressView().accessibilityLabel("Downloading") }
                            else { Icon("attachment").foregroundStyle(Theme.secondary) }
                            Text(name).lagomFont(.snippet, .medium).lineLimit(1).truncationMode(.middle)
                            Text(size).lagomFont(.small).monospacedDigit().foregroundStyle(Theme.secondary)
                        }
                        .padding(.horizontal, 14).frame(minHeight: 44)
                        .background(Theme.subtle, in: RoundedRectangle(cornerRadius: Theme.Radius.attachment))
                        .overlay(RoundedRectangle(cornerRadius: Theme.Radius.attachment).strokeBorder(Theme.line))
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .disabled(downloading)
                    .accessibilityLabel("Open \(name), \(size)")
                }
            }
        }
        .scrollIndicators(.hidden)
        .accessibilityLabel("Attachments")
    }
}

struct MailHTML: UIViewRepresentable {
    let document: String
    let messageID: Int64
    let core: LagomCore

    func makeCoordinator() -> Coordinator { Coordinator(messageID: messageID, core: core) }

    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        configuration.setURLSchemeHandler(context.coordinator.images, forURLScheme: "lagom-cid")
        configuration.defaultWebpagePreferences.allowsContentJavaScript = false
        configuration.preferences.javaScriptCanOpenWindowsAutomatically = false
        configuration.websiteDataStore = .nonPersistent()
        let view = WKWebView(frame: .zero, configuration: configuration)
        view.navigationDelegate = context.coordinator
        // A long-press preview would fetch the link outside the navigation delegate and the CSP.
        view.allowsLinkPreview = false
        view.isOpaque = false
        view.backgroundColor = .clear
        return view
    }

    func updateUIView(_ view: WKWebView, context: Context) {
        guard context.coordinator.document != document else { return }
        context.coordinator.document = document
        context.coordinator.initialLoad = true
        view.loadHTMLString(document, baseURL: nil)
    }

    @MainActor final class Coordinator: NSObject, WKNavigationDelegate {
        let images: InlineImages
        init(messageID: Int64, core: LagomCore) { images = InlineImages(messageID: messageID, core: core) }
        var document = ""
        var initialLoad = false

        func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction, decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
            if initialLoad && action.navigationType == .other && action.targetFrame?.isMainFrame == true && action.request.url?.absoluteString == "about:blank" {
                initialLoad = false
                decisionHandler(.allow)
                return
            }
            decisionHandler(.cancel)
            if action.navigationType == .linkActivated, let url = action.request.url,
               ["http", "https", "mailto"].contains(url.scheme?.lowercased() ?? "") {
                UIApplication.shared.open(url)
            }
        }
    }
}

@MainActor final class InlineImages: NSObject, WKURLSchemeHandler {
    let messageID: Int64
    let core: LagomCore
    private var tasks: [ObjectIdentifier: Task<Void, Never>] = [:]

    init(messageID: Int64, core: LagomCore) {
        self.messageID = messageID
        self.core = core
    }

    func webView(_ webView: WKWebView, start urlSchemeTask: WKURLSchemeTask) {
        let key = ObjectIdentifier(urlSchemeTask)
        guard urlSchemeTask.request.httpMethod == "GET", let url = urlSchemeTask.request.url else {
            urlSchemeTask.didFailWithError(URLError(.unsupportedURL))
            return
        }
        tasks[key] = Task { [weak self] in
            guard let self else { return }
            defer { tasks[key] = nil }
            do {
                let part = try await core.inlineResource(id: messageID, request: url.absoluteString)
                guard !Task.isCancelled else { return }
                guard let response = HTTPURLResponse(url: url, statusCode: 200, httpVersion: nil, headerFields: [
                    "Content-Type": part.mimeType, "Content-Length": String(part.data.count),
                    "Content-Security-Policy": "default-src 'none'; sandbox",
                    "X-Content-Type-Options": "nosniff", "Cache-Control": "no-store"
                ]) else { throw URLError(.badServerResponse) }
                urlSchemeTask.didReceive(response)
                urlSchemeTask.didReceive(part.data)
                urlSchemeTask.didFinish()
            } catch {
                guard !Task.isCancelled else { return }
                urlSchemeTask.didFailWithError(error)
            }
        }
    }

    func webView(_ webView: WKWebView, stop urlSchemeTask: WKURLSchemeTask) {
        tasks.removeValue(forKey: ObjectIdentifier(urlSchemeTask))?.cancel()
    }
}
