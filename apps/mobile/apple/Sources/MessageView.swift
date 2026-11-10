import SwiftUI
import WebKit

struct MessageView: View {
    let message: MailMessage
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
                    Text("Remote images are blocked").font(.caption).foregroundStyle(Theme.secondary)
                }
            }.padding(.horizontal)
            if let html = message.htmlBody, !showText {
                MailHTML(document: mailDocument(html: html, dark: scheme == .dark))
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

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
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

    final class Coordinator: NSObject, WKNavigationDelegate {
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
