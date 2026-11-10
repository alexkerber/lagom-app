import SwiftUI
import WebKit

struct MessageView: View {
    let message: MailMessage
    let model: MailModel
    @Environment(\.colorScheme) private var scheme
    @State private var showText = false
    @ScaledMetric(relativeTo: .title2) private var subjectSize = 20.0
    @ScaledMetric(relativeTo: .body) private var bodySize = 14.0

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            VStack(alignment: .leading, spacing: 8) {
                Text(message.row.subject).font(.system(size: subjectSize, weight: .semibold))
                Text(message.row.senderName).fontWeight(.medium)
                Text(message.row.senderAddress).font(.callout).foregroundStyle(Theme.secondary).textSelection(.enabled)
                Text(Date(timeIntervalSince1970: TimeInterval(message.row.dateUnix)), style: .date)
                    .font(.caption).foregroundStyle(Theme.secondary)
                if message.htmlBody != nil {
                    Toggle("Plain text", isOn: $showText).font(.caption)
                    if model.remoteImagesID != message.row.id {
                        Button("Load images") { model.loadRemoteImages(id: message.row.id) }.font(.caption)
                        Text("Remote images are blocked").font(.caption).foregroundStyle(Theme.secondary)
                    }
                }
            }.padding(.horizontal)
            if let html = message.htmlBody, !showText {
                if let core = model.core {
                    MailHTML(document: mailDocument(html: html, dark: scheme == .dark, messageId: message.row.id,
                                                    remote: model.remoteImagesID == message.row.id),
                             messageID: message.row.id, core: core)
                        .id("\(message.row.id)-\(model.inlineRevision)")
                }
            } else {
                ScrollView {
                    Text(message.textBody).font(.system(size: bodySize)).textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal)
                }
            }
            // shortcut: attachment metadata only in v1; add downloads when mobile file handling is implemented.
            if !message.attachments.isEmpty {
                ScrollView(.horizontal) {
                    HStack {
                        ForEach(message.attachments, id: \.section) { part in
                            Label("\(part.filename) · \(ByteCountFormatter.string(fromByteCount: Int64(part.size), countStyle: .file))", systemImage: "paperclip")
                                .font(.caption).padding(8).background(Theme.subtle, in: RoundedRectangle(cornerRadius: 10))
                        }
                    }.padding(.horizontal)
                }.accessibilityLabel("Attachment information")
            }
        }
        .padding(.top).foregroundStyle(Theme.ink).background(Theme.raised)
        .navigationBarTitleDisplayMode(.inline)
        .onChange(of: message.row.id) { _, _ in showText = false }
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
