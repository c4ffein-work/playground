import FeedKit
import SwiftUI
import WebKit

/// One article. Opening it marks it read.
struct ArticleView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openURL) private var openURL

    let articleID: Article.ID

    var body: some View {
        if let article = model.article(id: articleID) {
            ArticleWebView(html: model.page(for: article)) { openURL($0) }
                .ignoresSafeArea(edges: .bottom)
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItemGroup(placement: .topBarTrailing) {
                        Button {
                            model.toggleRead(article)
                        } label: {
                            if model.isRead(article) {
                                Label("Mark as unread", systemImage: "circle")
                            } else {
                                Label("Mark as read", systemImage: "circle.fill")
                            }
                        }
                        if let url = article.webURL {
                            ShareLink(item: url)
                            Button {
                                openURL(url)
                            } label: {
                                Label("Open in Safari", systemImage: "safari")
                            }
                        }
                    }
                }
                .onAppear { model.setRead(true, article) }
        } else {
            // Its feed was removed while it was open.
            ContentUnavailableView("This article is gone", systemImage: "tray")
        }
    }
}

/// Shows the page the core made for an article, and nothing else.
///
/// What a feed writes never gets here as it is (see `AppModel.page(for:)`), and on top of that:
/// - JavaScript is off;
/// - the page has no origin (`about:blank`), no cookie and no storage, and its own policy lets it load
///   nothing but, when the user asked for them, images over https;
/// - the view goes nowhere: a tapped link is handed to `open` (which hands http(s) links to Safari),
///   and any other navigation is refused.
struct ArticleWebView: UIViewRepresentable {
    let html: String
    let open: (URL) -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(open: open)
    }

    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        configuration.defaultWebpagePreferences.allowsContentJavaScript = false
        configuration.websiteDataStore = .nonPersistent()
        let view = WKWebView(frame: .zero, configuration: configuration)
        view.navigationDelegate = context.coordinator
        view.allowsLinkPreview = false
        view.isOpaque = false
        view.backgroundColor = .clear
        return view
    }

    func updateUIView(_ view: WKWebView, context: Context) {
        context.coordinator.open = open
        // Marking the article read redraws the view: the same page is not loaded again (it would scroll back up).
        guard context.coordinator.shown != html else { return }
        context.coordinator.shown = html
        view.loadHTMLString(html, baseURL: nil)
    }

    final class Coordinator: NSObject, WKNavigationDelegate {
        var open: (URL) -> Void
        var shown: String?

        init(open: @escaping (URL) -> Void) {
            self.open = open
        }

        func webView(
            _ webView: WKWebView, decidePolicyFor action: WKNavigationAction,
            decisionHandler: @escaping (WKNavigationActionPolicy) -> Void
        ) {
            // The page itself, as loaded by `loadHTMLString`.
            if action.navigationType == .other, action.request.url?.absoluteString == "about:blank" {
                return decisionHandler(.allow)
            }
            if action.navigationType == .linkActivated, let url = action.request.url.flatMap({ openableURL($0) }) {
                open(url)
            }
            decisionHandler(.cancel)
        }
    }
}
