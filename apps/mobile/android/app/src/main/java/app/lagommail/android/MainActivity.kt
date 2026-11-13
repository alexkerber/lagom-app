package app.lagommail.android

import android.Manifest
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
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.material3.windowsizeclass.*
import androidx.compose.runtime.*
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.compose.LifecycleEventEffect
import androidx.lifecycle.viewmodel.compose.viewModel
import app.lagommail.core.LagomCore
import app.lagommail.core.SendAttachment
import app.lagommail.core.installSecretStore
import app.lagommail.core.parseMailto
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File

class LagomApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        MailRefreshWorker.schedule(this)
    }
    private val credentials by lazy { installSecretStore(AndroidSecretStore(this)); true }
    val core: LagomCore by lazy {
        check(credentials)
        LagomCore.open(File(noBackupFilesDir, "mail").absolutePath)
    }
    /** A cancelled background handle stays cancelled, so each run gets its own; the stores are shared. */
    fun backgroundCore(): LagomCore {
        check(credentials)
        return LagomCore.open(File(noBackupFilesDir, "mail").absolutePath)
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

    private fun requestNotificationPermission() {
        if (Build.VERSION.SDK_INT < 33 || checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED) return
        val settings = getSharedPreferences("notifications", MODE_PRIVATE)
        if (settings.getBoolean("requested", false)) return
        settings.edit().putBoolean("requested", true).apply()
        notificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
    }

    @OptIn(ExperimentalMaterial3WindowSizeClassApi::class)
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        readNotification(intent)
        // A recreated activity already opened this share.
        if (savedInstanceState == null && intent.action in shareActions) shareIntent = intent
        enableEdgeToEdge()
        setContent {
            val model: MailModel = viewModel(factory = object : ViewModelProvider.Factory {
                @Suppress("UNCHECKED_CAST")
                override fun <T : ViewModel> create(modelClass: Class<T>): T = MailModel(open = {
                    withContext(Dispatchers.IO) { (application as LagomApplication).core }
                }, attachmentDirectory = File(cacheDir, "attachments"), sendLater = { SendWorker.schedule(applicationContext) }) as T
            })
            LifecycleEventEffect(Lifecycle.Event.ON_START) { model.perform { model.setForeground(true) } }
            LifecycleEventEffect(Lifecycle.Event.ON_STOP) { model.perform { model.setForeground(false) } }
            val state by lifecycle.currentStateAsState()
            LaunchedEffect(state, model.accounts.isEmpty()) {
                if (state.isAtLeast(Lifecycle.State.RESUMED) && model.accounts.isNotEmpty()) requestNotificationPermission()
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
            LagomTheme {
                MailScreen(model, size.widthSizeClass != WindowWidthSizeClass.Compact && size.heightSizeClass != WindowHeightSizeClass.Compact)
            }
        }
    }
}

@Composable internal fun LagomTheme(content: @Composable () -> Unit) {
    val dark = isSystemInDarkTheme()
    val colors = if (dark) darkColorScheme(
        primary = Color(0xFF8AB0F5), onPrimary = Color(0xFF1E1E1E),
        background = Color(0xFF1E1E1E), onBackground = Color(0xFFF2F2EE),
        surface = Color(0xFF2C2C2C), onSurface = Color(0xFFF2F2EE),
        surfaceVariant = Color(0xFF262626), onSurfaceVariant = Color(0xFF9B9B95),
        outline = Color(0xFF9B9B95), outlineVariant = Color(0xFF3A3A3A),
        secondary = Color(0xFFF5C22E), onSecondary = Color(0xFF1E1E1E),
        tertiary = Color(0xFF4CC38A), onTertiary = Color(0xFF1E1E1E), error = Color(0xFFF87171),
    ) else lightColorScheme(
        primary = Color(0xFF2A5BB8), onPrimary = Color.White,
        background = Color.White, onBackground = Color(0xFF111110),
        surface = Color.White, onSurface = Color(0xFF111110),
        surfaceVariant = Color(0xFFF5F5F3), onSurfaceVariant = Color(0xFF5F5F5A),
        outline = Color(0xFF5F5F5A), outlineVariant = Color(0xFFE6E6E2),
        secondary = Color(0xFF94640A), onSecondary = Color.White,
        tertiary = Color(0xFF1E6B4E), onTertiary = Color.White, error = Color(0xFFC62828),
    )
    MaterialTheme(colorScheme = colors, shapes = Shapes(small = RoundedCornerShape(8.dp), medium = RoundedCornerShape(12.dp)), content = content)
}
