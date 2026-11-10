package app.lagommail.android

import android.content.ActivityNotFoundException
import android.content.Intent
import android.webkit.CookieManager
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.viewinterop.AndroidView
import app.lagommail.core.mailDocument
import java.io.ByteArrayInputStream
import java.net.URI

internal fun externalMailLink(url: String): Boolean = try {
    URI(url).scheme?.lowercase() in setOf("http", "https", "mailto")
} catch (_: Exception) { false }

@Composable internal fun MailHtml(html: String) {
    val document = mailDocument(html, isSystemInDarkTheme())
    AndroidView(modifier = Modifier.fillMaxSize(), factory = { context ->
        WebView(context).apply {
            settings.javaScriptEnabled = false
            settings.javaScriptCanOpenWindowsAutomatically = false
            settings.allowFileAccess = false
            settings.allowContentAccess = false
            settings.blockNetworkLoads = true
            settings.blockNetworkImage = true
            settings.domStorageEnabled = false
            settings.cacheMode = WebSettings.LOAD_NO_CACHE
            settings.mixedContentMode = WebSettings.MIXED_CONTENT_NEVER_ALLOW
            CookieManager.getInstance().setAcceptCookie(false)
            CookieManager.getInstance().setAcceptThirdPartyCookies(this, false)
            webViewClient = object : WebViewClient() {
                override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest): WebResourceResponse =
                    WebResourceResponse("text/plain", "UTF-8", ByteArrayInputStream(byteArrayOf()))
                override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
                    if (request.isForMainFrame && request.hasGesture() && externalMailLink(request.url.toString())) {
                        try { context.startActivity(Intent(Intent.ACTION_VIEW, request.url).addCategory(Intent.CATEGORY_BROWSABLE)) }
                        catch (_: ActivityNotFoundException) { /* The message remains readable without a browser. */ }
                    }
                    return true
                }
            }
        }
    }, update = { view ->
        if (view.tag != document) {
            view.tag = document
            view.loadDataWithBaseURL(null, document, "text/html", "UTF-8", null)
        }
    }, onRelease = { view -> view.stopLoading(); view.destroy() })
}
