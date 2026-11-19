package app.lagommail.android

import androidx.activity.compose.BackHandler
import androidx.compose.animation.*
import androidx.compose.animation.core.EaseOut
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.boundsInWindow
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInWindow
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.em
import androidx.compose.ui.unit.sp
import app.lagommail.core.SignInProvider
import app.lagommail.core.signInAvailable

/** The first run, the same on iOS: Welcome, then the kind of account, then its form; once an account is in, one
 *  notifications step. Every step stands on the app icon's sky, blue where the words are. Copy matches iOS word for word. */
private enum class Step { WELCOME, CHOOSE, FORM, NOTIFICATIONS }

@Composable internal fun Onboarding(model: MailModel, askNotifications: Boolean, demo: Boolean, answer: (allow: Boolean) -> Unit) {
    var choosing by rememberSaveable { mutableStateOf(false) }
    // Demo lists both providers so the step can be captured; a tap there only shows the core's refusal.
    val providers = remember { listOf(SignInProvider.GOOGLE to "Google", SignInProvider.MICROSOFT to "Microsoft").filter { demo || signInAvailable(it.first) } }
    val form = model.accountForm
    val step = when {
        askNotifications -> Step.NOTIFICATIONS
        form != null -> Step.FORM
        choosing -> Step.CHOOSE
        else -> Step.WELCOME
    }
    BackHandler(step == Step.CHOOSE) { choosing = false }
    // The sky's top and height and the bottom of the step's heading, in the window, so the blue reaches past the white words.
    var skyTop by remember { mutableFloatStateOf(0f) }
    var skyHeight by remember { mutableFloatStateOf(0f) }
    var headBottom by remember { mutableFloatStateOf(Float.NaN) }
    val margin = with(LocalDensity.current) { 24.dp.toPx() }
    val band by animateFloatAsState(if (skyHeight > 0 && !headBottom.isNaN()) ((headBottom + margin - skyTop) / skyHeight).coerceIn(0.2f, 0.95f) else 0.4f,
        tween(250, easing = EaseOut), label = "sky")
    val c = lagom
    Box(Modifier.fillMaxSize()
        .onGloballyPositioned { skyTop = it.boundsInWindow().top; skyHeight = it.size.height.toFloat() }
        .background(Brush.verticalGradient(0f to c.skyTop, band to c.skyTop, (band + 1f) / 2f to c.skyMid, 1f to c.skyBottom))) {
        // motion.md: forward steps arrive from the end, back from the start; 250 ms ease-out with a fade. "Remove
        // animations" in the system settings leaves only the change.
        AnimatedContent(step, Modifier.fillMaxSize(), label = "onboarding", transitionSpec = {
            val forward = targetState.ordinal > initialState.ordinal
            val move = tween<IntOffset>(250, easing = EaseOut)
            (fadeIn(tween(250, easing = EaseOut)) + slideInHorizontally(move) { if (forward) it / 8 else -it / 8 }) togetherWith
                (fadeOut(tween(150, easing = EaseOut)) + slideOutHorizontally(move) { if (forward) -it / 8 else it / 8 })
        }) { shown ->
            val head = Modifier.onGloballyPositioned { if (shown == step) headBottom = it.positionInWindow().y + it.size.height }
            when (shown) {
                Step.WELCOME -> Welcome(model, head) { choosing = true }
                Step.CHOOSE -> ChooseAccount(model, providers, head) { choosing = false }
                Step.FORM -> model.accountForm?.let { AddAccountScreen(model, it, showProviders = false) { model.accountForm = null } }
                Step.NOTIFICATIONS -> Notifications(head, answer)
            }
        }
    }
}

/** The app icon, as the launcher draws it: the adaptive layers cropped to their visible middle. */
@Composable internal fun AppIcon(size: Dp) {
    Box(Modifier.size(size).clip(RoundedCornerShape(size * 0.24f)), contentAlignment = Alignment.Center) {
        // Adaptive icons show the middle 72 of their 108 units.
        Image(painterResource(R.drawable.icon_background), null, Modifier.requiredSize(size * 1.5f))
        Image(painterResource(R.drawable.icon_foreground), null, Modifier.requiredSize(size * 1.5f))
    }
}

/** A step's page: content in a 440 dp column (392 inside its margins), scrolling under large text, its buttons at the
 *  foot. On a tablet the column stops at 800 dp, so the buttons stay near the content. */
@Composable private fun StepPage(top: (@Composable () -> Unit)? = null, centred: Boolean = true, buttons: @Composable ColumnScope.() -> Unit,
    content: @Composable ColumnScope.() -> Unit) {
    Column(Modifier.fillMaxSize().systemBarsPadding()) {
        top?.invoke()
        Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
            Column(Modifier.fillMaxWidth().heightIn(max = 800.dp).fillMaxHeight()) {
                BoxWithConstraints(Modifier.weight(1f).fillMaxWidth()) {
                    Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).heightIn(min = maxHeight)
                        .padding(start = 24.dp, end = 24.dp, top = if (top != null) 0.dp else 24.dp, bottom = 24.dp),
                        verticalArrangement = if (centred) Arrangement.Center else Arrangement.Top, horizontalAlignment = Alignment.CenterHorizontally) {
                        Column(Modifier.widthIn(max = 392.dp).fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally, content = content)
                    }
                }
                Column(Modifier.widthIn(max = 440.dp).fillMaxWidth().align(Alignment.CenterHorizontally).padding(horizontal = 24.dp).padding(bottom = 16.dp),
                    verticalArrangement = Arrangement.spacedBy(6.dp), horizontalAlignment = Alignment.CenterHorizontally, content = buttons)
            }
        }
    }
}

/** The site's display line: Hauora Bold with its tight tracking, growing with the font scale. */
@Composable private fun DisplayTitle(text: String, size: TextUnit, modifier: Modifier = Modifier, textAlign: TextAlign = TextAlign.Center) =
    Text(text, modifier.semantics { heading() }, color = lagom.skyInk, fontFamily = DisplayFace, fontWeight = FontWeight.Bold, fontSize = size,
        letterSpacing = DisplayTracking.em, lineHeight = size * 1.1f, textAlign = textAlign)

@Composable private fun SkyText(text: String, modifier: Modifier = Modifier, size: TextUnit = TextSize.body, textAlign: TextAlign = TextAlign.Center) =
    Text(text, modifier, color = lagom.skyInk, fontSize = size, lineHeight = size * Leading, textAlign = textAlign)

/** The intro's one way on: a full-width pill, ink on the light end of the sky, as the site's primary button. */
@Composable private fun Pill(text: String, fill: Color = lagom.ink, ink: Color = lagom.bg, enabled: Boolean = true,
    mark: (@Composable () -> Unit)? = null, onClick: () -> Unit) {
    Row(Modifier.fillMaxWidth().heightIn(min = 52.dp).clip(CircleShape).background(fill)
        .clickable(enabled = enabled, role = Role.Button, onClick = onClick).alpha(if (enabled) 1f else 0.5f)
        .padding(horizontal = 20.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterHorizontally)) {
        mark?.invoke()
        Text(text, color = ink, fontSize = TextSize.body, lineHeight = TextSize.body * Leading, fontWeight = FontWeight.SemiBold, textAlign = TextAlign.Center)
    }
}

@Composable private fun Welcome(model: MailModel, head: Modifier, start: () -> Unit) {
    StepPage(buttons = {
        Pill("Get started", enabled = model.core != null, onClick = start)
        // The store could not open: the only way on is to try again.
        if (model.core == null && !model.loading) Pill("Try again", fill = Color.Transparent, ink = lagom.ink) { model.perform { model.setForeground(true) } }
        Text("No tracking. Your mail goes only to your server.", Modifier.padding(top = 6.dp), color = lagom.ink2,
            style = MaterialTheme.typography.bodySmall, textAlign = TextAlign.Center)
    }) {
        Column(head, horizontalAlignment = Alignment.CenterHorizontally) {
            Box(Modifier.shadow(20.dp, RoundedCornerShape(104.dp * 0.24f))) { AppIcon(104.dp) }
            DisplayTitle("lagom mail", 44.sp, Modifier.padding(top = 28.dp))
            SkyText("Calm email for a clear day.", Modifier.padding(top = 8.dp), TextSize.title)
        }
    }
}

@Composable private fun ChooseAccount(model: MailModel, providers: List<Pair<SignInProvider, String>>, head: Modifier, back: () -> Unit) {
    val context = LocalContext.current
    val c = lagom
    StepPage(top = {
        Row(Modifier.fillMaxWidth().height(BarHeight).padding(start = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            LagomIconButton("Back", R.drawable.ic_chevron_left, tint = c.skyInk, onClick = back)
        }
    }, centred = false, buttons = {}) {
        Column(head.fillMaxWidth()) {
            DisplayTitle("Choose your mail", 34.sp, textAlign = TextAlign.Start)
            SkyText("lagom connects straight to your mail server, with nothing in between.", Modifier.padding(top = 8.dp), textAlign = TextAlign.Start)
        }
        // Choose account's card: the browser sign-ins first, then the mail apps that use the mail server form.
        Column(Modifier.padding(top = 28.dp).fillMaxWidth().shadow(24.dp, RoundedCornerShape(Radius.card), ambientColor = Color.Black.copy(alpha = 0.1f),
            spotColor = Color.Black.copy(alpha = 0.1f)).clip(RoundedCornerShape(Radius.card)).background(c.raised).padding(20.dp),
            horizontalAlignment = Alignment.CenterHorizontally) {
            // Google asks for its full button text and its "G" on white.
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                providers.forEach { (provider, name) ->
                    val logo = if (provider == SignInProvider.GOOGLE) R.drawable.logo_google else R.drawable.logo_microsoft
                    Pill("Continue with $name", fill = c.subtle, ink = c.ink, mark = {
                        Box(Modifier.size(30.dp).clip(CircleShape).background(Color.White), contentAlignment = Alignment.Center) {
                            Image(painterResource(logo), null, Modifier.size(18.dp))
                        }
                    }) { model.perform { openSignIn(context, requireNotNull(model.core).beginSignIn(provider).url) } }
                }
            }
            MonoLabel("Other mail", Modifier.padding(top = 22.dp, bottom = 14.dp))
            // Four across; two across from 1.3× font scale, so no name breaks inside a word.
            val columns = if (LocalDensity.current.fontScale >= 1.3f) 2 else 4
            Column(verticalArrangement = Arrangement.spacedBy(16.dp)) {
                listOf("iCloud" to R.drawable.ic_cloud, "Fastmail" to R.drawable.ic_inbox, "Yahoo" to R.drawable.ic_mail,
                    "Other (IMAP)" to R.drawable.ic_settings).chunked(columns).forEach { row ->
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        row.forEach { (name, icon) -> Tile(name, icon, Modifier.weight(1f)) { model.accountForm = AccountForm() } }
                    }
                }
            }
            Row(Modifier.padding(top = 22.dp).semantics(mergeDescendants = true) {}, horizontalArrangement = Arrangement.spacedBy(6.dp),
                verticalAlignment = Alignment.CenterVertically) {
                LagomIcon(R.drawable.ic_access, size = 14.dp)
                Text("Passwords stay on this device.", color = c.ink2, style = MaterialTheme.typography.bodySmall)
            }
        }
    }
}

/** A mail app as an icon tile with its name under it, as Spark lists providers. The address decides the servers. */
@Composable private fun Tile(title: String, icon: Int, modifier: Modifier, onClick: () -> Unit) {
    val c = lagom
    Column(modifier.clip(RoundedCornerShape(Radius.control)).clickable(role = Role.Button, onClick = onClick).padding(vertical = 4.dp),
        horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Box(Modifier.size(56.dp).clip(RoundedCornerShape(Radius.panel)).background(c.subtle), contentAlignment = Alignment.Center) {
            LagomIcon(icon, tint = c.accent, size = 24.dp)
        }
        Text(title, color = c.ink, style = MaterialTheme.typography.labelMedium, textAlign = TextAlign.Center)
    }
}

@Composable private fun Notifications(head: Modifier, answer: (Boolean) -> Unit) {
    val c = lagom
    StepPage(buttons = {
        Pill("Allow") { answer(true) }
        Pill("Not now", fill = Color.Transparent, ink = c.ink) { answer(false) }
    }) {
        Column(head, horizontalAlignment = Alignment.CenterHorizontally) {
            Box(Modifier.clip(RoundedCornerShape(Radius.card)).background(c.skyInk.copy(alpha = 0.16f)).padding(22.dp)) {
                LagomIcon(R.drawable.ic_inbox, tint = c.skyInk, size = 30.dp)
            }
            DisplayTitle("New mail, when it arrives", 30.sp, Modifier.padding(top = 24.dp))
            SkyText("lagom tells you about new mail in your Inbox. Nothing else.", Modifier.padding(top = 8.dp))
        }
    }
}
