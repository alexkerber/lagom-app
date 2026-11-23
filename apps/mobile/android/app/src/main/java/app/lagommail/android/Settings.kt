package app.lagommail.android

import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.Uri
import android.provider.Settings
import androidx.activity.compose.BackHandler
import androidx.annotation.DrawableRes
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.lagommail.core.MailAccount

/** The Mac's Settings page (Alex 2026-11-20: one Settings page on every platform): Accounts, a row per account that
 *  opens its page (Your name, Signature, Remove account), then General and About. From the mailbox menu's last item;
 *  the same page as iOS, without keyboard shortcuts. */
@Composable internal fun SettingsScreen(model: MailModel, close: () -> Unit) {
    val context = LocalContext.current
    var open by rememberSaveable { mutableStateOf<String?>(null) }
    val index = model.accounts.indexOfFirst { it.id == open }
    // A removed account's page closes: the list shows, and Back leaves Settings.
    BackHandler { if (index >= 0) open = null else close() }
    var removing by remember { mutableStateOf<MailAccount?>(null) }
    // The open editor survives rotation by its account's id; its text is saved in the dialog.
    var signing by rememberSaveable { mutableStateOf<String?>(null) }
    var naming by rememberSaveable { mutableStateOf<String?>(null) }
    var failed by remember { mutableStateOf<String?>(null) }
    fun open(intent: Intent) = try { context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) }
        catch (_: ActivityNotFoundException) { failed = "No app on this phone can open this." }
    // A Surface, not a Column: it takes every touch, so nothing reaches the list underneath.
    Surface(Modifier.fillMaxSize(), color = lagom.bg) { Column(Modifier.fillMaxSize()) {
        TopBar(Modifier.statusBarsPadding(), start = if (index >= 0) 4.dp else 16.dp) {
            // Back as the Mail page's, an icon alone (Alex 2026-11-23: the same app bar everywhere).
            if (index >= 0) {
                LagomIconButton("Back to Settings", R.drawable.ic_chevron_left) { open = null }
                Spacer(Modifier.weight(1f))
            } else BarTitle("Settings", Modifier.weight(1f))
            LagomIconButton("Close", R.drawable.ic_close, onClick = close)
        }
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).navigationBarsPadding().padding(16.dp),
            horizontalAlignment = Alignment.CenterHorizontally) {
            // The Mac's 600 dp column, centred on a tablet.
            Column(Modifier.widthIn(max = 600.dp).fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(32.dp)) {
                if (index >= 0) AccountPage(model.accounts[index], accountAccent(index), { naming = it.id }, { signing = it.id }, { removing = it })
                else {
                    Group("Accounts", "The email addresses in lagom. Only you see them.") {
                        model.accounts.forEachIndexed { at, account ->
                            if (at > 0) Hairline()
                            SettingsRow(account.email, detail = accountKind(account), mark = { Avatar(account, accountAccent(at), 32) }) { open = account.id }
                        }
                        if (model.accounts.isNotEmpty()) Hairline()
                        SettingsRow("Add account", mark = { LagomIcon(R.drawable.ic_account_add) }, chevron = false, enabled = model.core != null) {
                            model.accountForm = AccountForm(choosing = true)
                        }
                    }
                    Group("General") {
                        // The system owns the switch on a phone; the row opens it.
                        SettingsRow("Notifications for new mail", icon = R.drawable.ic_external) {
                            open(Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName))
                        }
                    }
                    Group("About", footer = {
                        val version = context.packageManager.getPackageInfo(context.packageName, 0).versionName.orEmpty()
                        Text("lagom $version", Modifier.padding(top = 8.dp), color = lagom.ink2, style = MaterialTheme.typography.bodySmall)
                        failed?.let { Text(it, Modifier.padding(top = 8.dp), color = lagom.danger, style = MaterialTheme.typography.bodySmall) }
                    }) {
                        About.forEachIndexed { at, (title, value, url) ->
                            if (at > 0) Hairline(16)
                            SettingsRow(title, value = value, icon = R.drawable.ic_external) { open(Intent(Intent.ACTION_VIEW, Uri.parse(url))) }
                        }
                    }
                }
            }
        }
    } }
    model.accounts.firstOrNull { it.id == signing }?.let { account -> SignatureDialog(model, account) { signing = null } }
    model.accounts.firstOrNull { it.id == naming }?.let { account -> NameDialog(model, account) { naming = null } }
    // The Mac's words: the question names the account, Keep is the way out.
    // Only on the open account's page: Back while the words load leaves the question behind.
    removing?.takeIf { it.id == open }?.let { account ->
        // The question waits for the account's words, so Remove never shows an early note.
        val note by produceState<String?>(null, account.id) { value = model.removalNote(account) }
        note?.let { words -> AlertDialog(onDismissRequest = { removing = null }, title = { Text("Remove ${account.email} from lagom?") },
            text = { Text(words) },
            confirmButton = { TextButton(onClick = { removing = null; model.perform { model.remove(account) } }) { Text("Remove", color = lagom.danger) } },
            dismissButton = { TextButton(onClick = { removing = null }) { Text("Keep") } }) }
    }
}

/** The Mac's About rows, in its order. */
private val About = listOf(
    Triple("Source code", null, "https://github.com/alexkerber/lagom-app"),
    Triple("Licence", "MIT", "https://github.com/alexkerber/lagom-app/blob/main/LICENSE"),
    Triple("Privacy", null, "https://lagommail.app/privacy.html"),
    Triple("Support", null, "https://lagommail.app/support.html"),
)

/** "Personal · Email and password", as the Mac's row. */
internal fun accountKind(account: MailAccount): String =
    listOf(account.displayName.takeIf { it != account.email }.orEmpty(), account.kind).filter { it.isNotEmpty() }.joinToString(" · ")

/** One account, as on the Mac: Your name and Signature, then Remove account. */
@Composable private fun AccountPage(account: MailAccount, color: Color, name: (MailAccount) -> Unit, sign: (MailAccount) -> Unit,
    remove: (MailAccount) -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(16.dp)) {
        Avatar(account, color, 48)
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(account.email, Modifier.semantics { heading() }, style = MaterialTheme.typography.titleLarge.copy(fontFamily = DisplayFace,
                fontWeight = FontWeight.Bold), maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (account.kind.isNotEmpty()) Text(account.kind, color = lagom.ink2, style = MaterialTheme.typography.bodySmall)
        }
    }
    Rows {
        val named = account.displayName != account.email
        SettingsRow("Your name", detail = if (named) account.displayName else "Shown to the people you write to.") { name(account) }
        Hairline(16)
        SettingsRow("Signature", detail = "Added to new mail, replies and forwards.") { sign(account) }
    }
    Column {
        Rows { SettingsRow("Remove account", mark = { LagomIcon(R.drawable.ic_account_remove, tint = lagom.danger) }, chevron = false,
            tint = lagom.danger) { remove(account) } }
        Text("Removes the account and its saved mail from this device. Your mail stays with your provider.", Modifier.padding(top = 8.dp),
            color = lagom.ink2, style = MaterialTheme.typography.bodySmall)
    }
}

/** A titled group: a mono label, an optional line, then its rows in one hairline frame. */
@Composable private fun Group(title: String, note: String? = null, footer: @Composable ColumnScope.() -> Unit = {},
    content: @Composable ColumnScope.() -> Unit) = Column {
    MonoLabel(title, Modifier.padding(bottom = if (note == null) 8.dp else 4.dp))
    note?.let { Text(it, Modifier.padding(bottom = 12.dp), color = lagom.ink2, style = MaterialTheme.typography.bodySmall) }
    Rows(content = content)
    footer()
}

/** One row: an optional mark, the title over an optional quiet line, a value, then a chevron or an icon. */
@Composable private fun SettingsRow(title: String, detail: String? = null, value: String? = null, mark: (@Composable () -> Unit)? = null,
    chevron: Boolean = true, @DrawableRes icon: Int? = null, tint: Color = lagom.ink, enabled: Boolean = true, onClick: () -> Unit) {
    Row(Modifier.fillMaxWidth().heightIn(min = if (mark == null && detail == null) 48.dp else 56.dp)
        .clickable(enabled = enabled, role = Role.Button, onClick = onClick).padding(horizontal = 16.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(16.dp)) {
        mark?.let { Box(Modifier.width(32.dp), contentAlignment = Alignment.Center) { it() } }
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(title, color = if (enabled) tint else lagom.ink2, fontSize = TextSize.body, lineHeight = TextSize.body * Leading,
                fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            detail?.let { Text(it, color = lagom.ink2, style = MaterialTheme.typography.bodySmall, maxLines = 2, overflow = TextOverflow.Ellipsis) }
        }
        value?.let { Text(it, color = lagom.ink2, style = MaterialTheme.typography.bodyLarge) }
        when {
            icon != null -> LagomIcon(icon, size = 14.dp)
            chevron -> LagomIcon(R.drawable.ic_chevron_right, size = 14.dp)
        }
    }
}

/** The Mac's account mark: the initial in the account's colour on a tint of it. */
@Composable private fun Avatar(account: MailAccount, color: Color, side: Int) {
    val name = account.displayName.ifBlank { account.email }
    Box(Modifier.size(side.dp).background(color.copy(alpha = 0.18f), CircleShape).clearAndSetSemantics {}, contentAlignment = Alignment.Center) {
        Text(name.firstOrNull { it.isLetterOrDigit() }?.uppercase().orEmpty(), color = color, fontFamily = DisplayFace,
            fontSize = (side * 0.42f).sp)
    }
}
