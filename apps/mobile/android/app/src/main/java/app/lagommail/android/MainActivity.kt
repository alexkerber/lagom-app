package app.lagommail.android

import android.Manifest
import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.provider.OpenableColumns
import androidx.core.content.IntentCompat
import android.os.Build
import androidx.activity.result.contract.ActivityResultContracts
import androidx.lifecycle.compose.currentStateAsState
import android.app.Application
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.SystemBarStyle
import androidx.activity.enableEdgeToEdge
import androidx.compose.animation.*
import androidx.compose.animation.core.EaseOut
import androidx.compose.animation.core.tween
import androidx.compose.foundation.border
import androidx.compose.foundation.background
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.material3.windowsizeclass.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.foundation.layout.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.foundation.text.TextAutoSize
import androidx.core.splashscreen.SplashScreen.Companion.installSplashScreen
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.compose.LifecycleEventEffect
import androidx.lifecycle.viewmodel.compose.viewModel
import app.lagommail.core.LagomCore
import app.lagommail.core.configureSignIn
import app.lagommail.core.SendAttachment
import app.lagommail.core.installSecretStore
import app.lagommail.core.parseMailto
import app.lagommail.core.seedDemo
import app.lagommail.core.seedDemoWithoutCalendar
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.drop
import java.time.LocalDate
import kotlinx.coroutines.withContext
import java.io.File

class LagomApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        MailRefreshWorker.schedule(this)
    }
    private val credentials by lazy {
        installSecretStore(AndroidSecretStore(this))
        // Token refresh in every sync needs the client IDs, also in a background worker.
        configureSignIn(packageName, BuildConfig.GOOGLE_CLIENT_ID, BuildConfig.MICROSOFT_CLIENT_ID)
        true
    }
    val core: LagomCore by lazy {
        check(credentials)
        LagomCore.open(File(noBackupFilesDir, "mail").absolutePath)
    }
    /** A cancelled background handle stays cancelled, so each run gets its own; the stores are shared. */
    fun backgroundCore(): LagomCore {
        check(credentials)
        return LagomCore.open(File(noBackupFilesDir, "mail").absolutePath)
    }
    /** A store of fixture mail in the cache, never the real one, for a debug [DebugLaunch.demo]. "welcome" starts empty each time. */
    fun demoCore(demo: String): LagomCore {
        check(credentials)
        val directory = File(cacheDir, if (demo == "mail" || demo == "notifications") "demo" else "demo-$demo")
        // Every demo launch starts from the same fixtures, whatever the last one marked done.
        directory.deleteRecursively()
        when (demo) {
            "welcome" -> {}
            // Calendar's empty states, for screenshots.
            "calendar-google", "calendar-granted", "calendar-none" -> seedDemoWithoutCalendar(directory.absolutePath, demo.removePrefix("calendar-"))
            else -> seedDemo(directory.absolutePath)
        }
        return LagomCore.open(directory.absolutePath)
    }
}

/**
 * Debug-only launch extras for screenshots without real accounts, as desktop's `LAGOM_DEMO` and `LAGOM_THEME`:
 * `--es lagomDemo mail|welcome|notifications|calendar-google|calendar-granted|calendar-none` and `--es lagomTheme light|dark`. A release build ignores both.
 */
internal data class DebugLaunch(val demo: String? = null, val dark: Boolean? = null)
internal fun debugLaunch(debug: Boolean, demo: String?, theme: String?): DebugLaunch =
    if (!debug) DebugLaunch() else DebugLaunch(demo?.takeIf { it in setOf("mail", "welcome", "notifications", "calendar-google", "calendar-granted", "calendar-none") },
        when (theme) { "light" -> false; "dark" -> true; else -> null })

/** The redirect of a browser sign-in, or null for any other link. The core then checks the exact URI and state. */
internal fun signInRedirect(action: String?, data: String?, appId: String): String? = data?.takeIf {
    action == Intent.ACTION_VIEW && (it.startsWith("com.googleusercontent.apps.") || it.startsWith("msauth.$appId:"))
}

/** Opens the sign-in page in a Custom Tab: the null session extra is what marks one. Other browsers open it in full. */
internal fun openSignIn(context: Context, url: String) {
    try {
        context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url))
            .putExtras(Bundle().apply { putBinder("android.support.customtabs.extra.SESSION", null) }))
    // The system's message would show the whole sign-in address.
    } catch (_: ActivityNotFoundException) { error("No browser found to sign in with.") }
}

/** Hands the redirect to the open MainActivity and closes the browser tab above it. */
class SignInRedirectActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // As an extra, not as data: the system logs an intent's data when it starts an activity.
        signInRedirect(intent.action, intent.dataString, packageName)?.let {
            startActivity(Intent(this, MainActivity::class.java).putExtra("signIn", it)
                .addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP))
        }
        finish()
    }
}

private const val SHARED_FILES_LIMIT = 25 * 1024 * 1024

/** Stops soon after `limit`, so a huge share never fills memory. InputStream.readNBytes needs API 33. */
private fun java.io.InputStream.readAtMost(limit: Int): ByteArray {
    val out = java.io.ByteArrayOutputStream()
    val buffer = ByteArray(64 * 1024)
    while (out.size() <= limit) { val read = read(buffer); if (read < 0) break; out.write(buffer, 0, read) }
    return out.toByteArray()
}

class MainActivity : ComponentActivity() {
    private var notificationTarget by mutableStateOf<MailTarget?>(null)
    private var signIn by mutableStateOf<String?>(null)
    private val notificationPermission = registerForActivityResult(ActivityResultContracts.RequestPermission()) {}

    private fun readNotification(intent: Intent) {
        val account = intent.getStringExtra("account") ?: return
        if (!intent.hasExtra("message")) return
        notificationTarget = MailTarget(account, intent.getLongExtra("message", 0))
        intent.removeExtra("account"); intent.removeExtra("message")
    }

    private var shareIntent by mutableStateOf<Intent?>(null)
    private val shareActions = setOf(Intent.ACTION_SENDTO, Intent.ACTION_VIEW, Intent.ACTION_SEND, Intent.ACTION_SEND_MULTIPLE)

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        readNotification(intent)
        signIn = intent.getStringExtra("signIn") ?: signIn
        if (intent.action in shareActions) shareIntent = intent
    }

    /** Runs off the main thread: shared files are read into the draft. */
    private fun readShare(intent: Intent): SharedMail? {
        fun list(name: String) = intent.getStringArrayExtra(name)?.toList().orEmpty()
        val mail = sharedMail(intent.action, intent.dataString, list(Intent.EXTRA_EMAIL), list(Intent.EXTRA_CC), list(Intent.EXTRA_BCC),
            intent.getStringExtra(Intent.EXTRA_SUBJECT), intent.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString()) { parseMailto(it) } ?: return null
        val uris = if (intent.action == Intent.ACTION_SEND_MULTIPLE) IntentCompat.getParcelableArrayListExtra(intent, Intent.EXTRA_STREAM, Uri::class.java).orEmpty()
            else listOfNotNull(IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java))
        var room = SHARED_FILES_LIMIT
        val files = uris.map { uri ->
            // Only another app's content: file:// or our own provider could attach lagom's private mail.
            require(uri.scheme == "content" && uri.authority != "$packageName.files") { "lagom can't attach this file." }
            val name = contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { if (it.moveToFirst()) it.getString(0) else null }
            val data = contentResolver.openInputStream(uri)?.use { it.readAtMost(room) } ?: error("lagom can't read this file.")
            require(data.size <= room) { "Shared files over 25 MB stay out of mail." }
            room -= data.size
            SendAttachment(safeAttachmentName(name ?: "attachment"), contentResolver.getType(uri) ?: "application/octet-stream", data)
        }
        return SharedMail(mail, files)
    }

    /** Android 13+ asks before an app may notify; lagom asks once, from the onboarding step. */
    private fun needsNotificationPermission() = Build.VERSION.SDK_INT >= 33 &&
        checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED &&
        !getSharedPreferences("notifications", MODE_PRIVATE).getBoolean("requested", false)

    private fun answerNotifications(allow: Boolean) {
        if (!needsNotificationPermission()) return
        getSharedPreferences("notifications", MODE_PRIVATE).edit().putBoolean("requested", true).apply()
        if (allow) notificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
    }

    @OptIn(ExperimentalMaterial3WindowSizeClassApi::class)
    override fun onCreate(savedInstanceState: Bundle?) {
        installSplashScreen()
        super.onCreate(savedInstanceState)
        readNotification(intent)
        // A recreated activity gets the same intent again; its redirect is already used.
        if (savedInstanceState == null) signIn = intent.getStringExtra("signIn")
        // A recreated activity already opened this share.
        if (savedInstanceState == null && intent.action in shareActions) shareIntent = intent
        val launch = debugLaunch(BuildConfig.DEBUG, intent.getStringExtra("lagomDemo"), intent.getStringExtra("lagomTheme"))
        // A forced debug theme sets the bar icons to match it; otherwise they follow the system.
        launch.dark?.let { dark ->
            val none = android.graphics.Color.TRANSPARENT
            val bars = if (dark) SystemBarStyle.dark(none) else SystemBarStyle.light(none, none)
            enableEdgeToEdge(bars, bars)
        } ?: enableEdgeToEdge()
        setContent {
            val model: MailModel = viewModel(factory = object : ViewModelProvider.Factory {
                @Suppress("UNCHECKED_CAST")
                override fun <T : ViewModel> create(modelClass: Class<T>): T = MailModel(open = {
                    val app = application as LagomApplication
                    withContext(Dispatchers.IO) { launch.demo?.let(app::demoCore) ?: app.core }
                }, attachmentDirectory = File(cacheDir, "attachments"), sendLater = { if (launch.demo == null) SendWorker.schedule(applicationContext) },
                    demo = launch.demo != null) as T
            })
            val planner: PlannerModel = viewModel()
            LifecycleEventEffect(Lifecycle.Event.ON_START) { model.perform { model.setForeground(true) }; planner.perform { planner.reload() } }
            LifecycleEventEffect(Lifecycle.Event.ON_STOP) { model.perform { model.setForeground(false) } }
            // Demo shows the step without touching the real permission or its setting.
            var notificationsAnswered by rememberSaveable { mutableStateOf(if (launch.demo != null) launch.demo != "notifications" else !needsNotificationPermission()) }
            LaunchedEffect(signIn, model.core) {
                val redirect = signIn
                if (redirect != null && model.core != null) { signIn = null; model.finishSignIn(redirect) }
            }
            LaunchedEffect(notificationTarget, model.core) {
                val target = notificationTarget
                if (target != null && model.core != null) {
                    try { model.openNotification(target) }
                    catch (cancelled: CancellationException) { throw cancelled }
                    catch (error: Exception) { model.error = error.mailMessage() }
                    if (notificationTarget == target) notificationTarget = null
                }
            }
            LaunchedEffect(shareIntent) {
                val intent = shareIntent ?: return@LaunchedEffect
                try {
                    val shared = withContext(Dispatchers.IO) { readShare(intent) }
                    if (shared == null) model.error = "lagom could not read this mail link." else model.openShared(shared)
                } catch (cancelled: CancellationException) { throw cancelled }
                catch (error: Exception) { model.error = error.message ?: error.mailMessage() }
                // Clearing the key earlier would cancel this effect mid-read; a newer share has its own run.
                finally { if (shareIntent == intent) shareIntent = null }
            }
            val size = calculateWindowSizeClass(this)
            LagomTheme(launch.dark ?: isSystemInDarkTheme()) {
                // Without an account there is no app shell yet: the welcome steps lead to adding one, then to notifications.
                val stage = when {
                    model.loading && model.accounts.isEmpty() -> Stage.LOADING
                    model.accounts.isEmpty() || !notificationsAnswered -> Stage.ONBOARDING
                    else -> Stage.HOME
                }
                AnimatedContent(stage, Modifier.fillMaxSize().background(lagom.bg), label = "stage",
                    transitionSpec = { fadeIn(tween(250, easing = EaseOut)) togetherWith fadeOut(tween(200, easing = EaseOut)) }) { shown ->
                    when (shown) {
                        Stage.LOADING -> Box(Modifier.fillMaxSize())
                        Stage.ONBOARDING -> Onboarding(model, askNotifications = model.accounts.isNotEmpty(), demo = launch.demo != null) { allow ->
                            if (launch.demo == null) answerNotifications(allow)
                            notificationsAnswered = true
                        }
                        Stage.HOME -> LagomHome(model, planner, size.widthSizeClass != WindowWidthSizeClass.Compact && size.heightSizeClass != WindowHeightSizeClass.Compact,
                            // Below 840 dp the sidebar would leave the reading pane too narrow; the bar stays.
                            sidebar = size.widthSizeClass == WindowWidthSizeClass.Expanded && size.heightSizeClass != WindowHeightSizeClass.Compact)
                    }
                }
                ErrorDialogs(model, planner)
            }
        }
    }
}

/** The phone page bar or tablet sidebar. The last page comes back at the next start. */
@Composable internal fun LagomHome(model: MailModel, planner: PlannerModel, twoPane: Boolean, sidebar: Boolean = false) {
    val settings = LocalContext.current.getSharedPreferences("settings", Context.MODE_PRIVATE)
    // A share or notification that landed during loading or onboarding opens on Mail, where it lives.
    var section by remember { mutableStateOf(if (model.draft != null || model.accountForm != null || model.selectedId != null) AppSection.MAIL
        else settings.getString("section", null)?.let { name -> AppSection.entries.find { it.name == name } } ?: AppSection.MAIL) }
    fun show(page: AppSection) { section = page; settings.edit().putString("section", page.name).apply() }
    /** A message Today asked to open; a tap on another page cancels it, and a half-open message with it. */
    var opening by remember { mutableStateOf<Job?>(null) }
    fun choose(page: AppSection) {
        opening?.takeIf { it.isActive && page != AppSection.MAIL }?.let { it.cancel(); model.clearSelection() }
        show(page)
    }
    // Compose, Add account and a newly opened message (a notification, a share) live on the Mail page.
    // Only changes count: after a rotation the page stays where it was.
    val mailOpen = model.draft != null || model.accountForm != null
    LaunchedEffect(Unit) {
        snapshotFlow { (model.draft != null || model.accountForm != null) to model.selectedId }.drop(1)
            .collect { (open, selected) -> if (open || selected != null) show(AppSection.MAIL) }
    }
    // Local tasks and events load as soon as the store opens, not after the first sync.
    LaunchedEffect(model.core) { model.core?.let { planner.core = it; planner.reload() } }
    // A page left open past midnight moves to the new day.
    LaunchedEffect(Unit) {
        var day = LocalDate.now()
        while (true) {
            delay(60_000)
            if (LocalDate.now() != day) { day = LocalDate.now(); planner.reload() }
        }
    }
    // Unread counts and Today's mail follow every mail reload and the end of a sync.
    LaunchedEffect(model.rows, model.syncing) { if (!model.syncing) planner.reloadMail() }
    // A sync fetches the calendar too.
    LaunchedEffect(model.syncing) { if (!model.syncing) planner.reloadAgenda() }
    LaunchedEffect(section) {
        if (section == AppSection.CALENDAR) planner.reloadAgenda()
        if (section == AppSection.TODAY) planner.reloadMail()
    }
    // A message reads full height on a phone; its back returns to the list and the bar.
    val bar = !mailOpen && !(section == AppSection.MAIL && model.selectedId != null && !twoPane)
    Scaffold(containerColor = if (sidebar) lagom.sidebar else lagom.bg,
        bottomBar = { if (!sidebar && bar) LagomBar(section, planner.unreadCount, ::choose) }) { padding ->
        Row(Modifier.fillMaxSize().padding(padding).consumeWindowInsets(padding)) {
            if (sidebar) LagomSidebar(model, section, planner.unreadCount, enabled = !mailOpen, choose = ::choose)
            // The page meets the sidebar at a straight edge and runs to the screen's edges, as desktop and iPad.
            Box(Modifier.weight(1f).fillMaxHeight().background(lagom.bg)) {
                when (section) {
                    AppSection.TODAY -> TodayScreen(planner, model, open = ::show) { id ->
                        show(AppSection.MAIL)
                        opening = model.perform { model.openInbox(id) }
                    }
                    AppSection.MAIL -> MailScreen(model, twoPane)
                    // The tablet layouts' breakpoint: a phone, also in landscape, keeps the one-day grid.
                    AppSection.CALENDAR -> CalendarScreen(planner, model, grid = twoPane)
                    AppSection.TODO -> TodoScreen(planner)
                }
            }
        }
    }
}

private enum class Stage { LOADING, ONBOARDING, HOME }

/** Errors from either model, over whichever stage shows. */
@Composable private fun ErrorDialogs(model: MailModel, planner: PlannerModel) {
    model.error?.let { error ->
        AlertDialog(onDismissRequest = { model.error = null }, title = { Text("Mail") }, text = { Text(error) },
            confirmButton = { TextButton(onClick = { model.error = null }) { Text("OK") } })
    }
    planner.error?.takeIf { model.error == null }?.let { error ->
        AlertDialog(onDismissRequest = { planner.error = null }, title = { Text("lagom") }, text = { Text(error) },
            confirmButton = { TextButton(onClick = { planner.error = null }) { Text("OK") } })
    }
}

/** The phone's page bar, iOS's system tab bar: Material's NavigationBar on the sidebar fill, lagom's icons, the grey
 *  selection with its icon in `sel_icon`, and a quiet unread count. */
@Composable private fun LagomBar(section: AppSection, unread: Int, choose: (AppSection) -> Unit) {
    val c = lagom
    NavigationBar(containerColor = c.sidebar, tonalElevation = 0.dp) {
        AppSection.entries.forEach { page ->
            val active = page == section
            NavigationBarItem(active, { choose(page) },
                // Material clears the icon slot's semantics when a label is present, so the badge says nothing to TalkBack.
                modifier = Modifier.semantics { if (page == AppSection.MAIL && unread > 0) stateDescription = "$unread unread" },
                icon = {
                    BadgedBox(badge = {
                        // Quiet, as on desktop and iOS: grey, not the Material red.
                        // A fixed size, as iOS's tab badge: scaled with the font it covered the icon. TalkBack reads the stateDescription.
                        if (page == AppSection.MAIL && unread > 0) Badge(containerColor = c.ink2, contentColor = c.sidebar) {
                            val size = with(LocalDensity.current) { 11.dp.toSp() }
                            Text("$unread", fontSize = size, lineHeight = size)
                        }
                    }) { LagomIcon(page.icon, tint = if (active) c.selIcon else c.ink2, size = 24.dp) }
                },
                // Large font scales shrink the label to fit its tab instead of cutting it ("Calenda").
                label = { Text(page.title, maxLines = 1, autoSize = TextAutoSize.StepBased(minFontSize = LocalTextStyle.current.fontSize / 2, maxFontSize = LocalTextStyle.current.fontSize)) },
                colors = NavigationBarItemDefaults.colors(indicatorColor = c.sel, selectedTextColor = c.ink, unselectedTextColor = c.ink2))
        }
    }
}
