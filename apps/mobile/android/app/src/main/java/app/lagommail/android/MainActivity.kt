package app.lagommail.android

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
import app.lagommail.core.installSecretStore
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File

class LagomApplication : Application() {
    private val credentials by lazy { installSecretStore(AndroidSecretStore(this)); true }
    val core: LagomCore by lazy {
        check(credentials)
        LagomCore.open(File(noBackupFilesDir, "mail").absolutePath)
    }
}

class MainActivity : ComponentActivity() {
    @OptIn(ExperimentalMaterial3WindowSizeClassApi::class)
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent {
            val model: MailModel = viewModel(factory = object : ViewModelProvider.Factory {
                @Suppress("UNCHECKED_CAST")
                override fun <T : ViewModel> create(modelClass: Class<T>): T = MailModel(open = {
                    withContext(Dispatchers.IO) { (application as LagomApplication).core }
                }, attachmentDirectory = File(cacheDir, "attachments")) as T
            })
            LifecycleEventEffect(Lifecycle.Event.ON_START) { model.perform { model.setForeground(true) } }
            LifecycleEventEffect(Lifecycle.Event.ON_STOP) { model.perform { model.setForeground(false) } }
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
