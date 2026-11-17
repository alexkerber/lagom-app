package app.lagommail.android

import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.Uri
import android.provider.Settings
import androidx.activity.compose.BackHandler
import androidx.annotation.DrawableRes
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp

/** iOS `SettingsView` on a phone: notifications and about, from the mailbox menu, where accounts already live. */
@Composable internal fun SettingsScreen(close: () -> Unit) {
    val context = LocalContext.current
    BackHandler(onBack = close)
    var failed by remember { mutableStateOf<String?>(null) }
    fun open(intent: Intent) = try { context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) }
        catch (_: ActivityNotFoundException) { failed = "No app on this phone can open this." }
    // A Surface, not a Column: it takes every touch, so nothing reaches the list underneath.
    Surface(Modifier.fillMaxSize(), color = lagom.bg) { Column(Modifier.fillMaxSize()) {
        TopBar(Modifier.statusBarsPadding()) {
            BarTitle("Settings", Modifier.weight(1f))
            LagomIconButton("Close", R.drawable.ic_close, onClick = close)
        }
        Column(Modifier.verticalScroll(rememberScrollState()).navigationBarsPadding().padding(horizontal = 16.dp, vertical = 16.dp),
            verticalArrangement = Arrangement.spacedBy(32.dp)) {
            Group("Notifications") {
                SettingsRow("Notification settings", R.drawable.ic_settings) {
                    open(Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName))
                }
            }
            Group("About") {
                SettingsRow("Privacy", R.drawable.ic_external) { open(Intent(Intent.ACTION_VIEW, Uri.parse("https://lagommail.app/privacy.html"))) }
                SettingsRow("Support", R.drawable.ic_external) { open(Intent(Intent.ACTION_VIEW, Uri.parse("https://lagommail.app/support.html"))) }
                val version = context.packageManager.getPackageInfo(context.packageName, 0).versionName.orEmpty()
                Text("lagom $version", Modifier.heightIn(min = 48.dp).wrapContentHeight(Alignment.CenterVertically), color = lagom.ink2,
                    style = MaterialTheme.typography.bodySmall)
                failed?.let { Text(it, color = lagom.danger, style = MaterialTheme.typography.bodySmall) }
            }
        }
    } }
}

@Composable private fun Group(title: String, content: @Composable ColumnScope.() -> Unit) = Column {
    MonoLabel(title, Modifier.padding(bottom = 8.dp))
    content()
}

@Composable private fun SettingsRow(title: String, @DrawableRes icon: Int, onClick: () -> Unit) {
    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).clickable(role = Role.Button, onClick = onClick),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        LagomIcon(icon, size = 16.dp)
        Text(title, style = MaterialTheme.typography.bodyLarge)
    }
}
