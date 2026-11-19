package app.lagommail.android

import androidx.activity.compose.BackHandler
import androidx.annotation.DrawableRes
import androidx.compose.animation.*
import androidx.compose.animation.core.EaseOut
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
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
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
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
 *  notifications step. Steps that do not apply are skipped. Copy matches iOS word for word. */
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
    fun addAccount() { if (providers.isEmpty()) model.accountForm = AccountForm() else choosing = true }
    // motion.md: forward steps arrive from the end, back from the start; 250 ms ease-out with a fade. "Remove
    // animations" in the system settings leaves only the change.
    AnimatedContent(step, Modifier.fillMaxSize(), label = "onboarding", transitionSpec = {
        val forward = targetState.ordinal > initialState.ordinal
        val move = tween<IntOffset>(250, easing = EaseOut)
        (fadeIn(tween(250, easing = EaseOut)) + slideInHorizontally(move) { if (forward) it / 8 else -it / 8 }) togetherWith
            (fadeOut(tween(150, easing = EaseOut)) + slideOutHorizontally(move) { if (forward) -it / 8 else it / 8 })
    }) { shown ->
        when (shown) {
            Step.WELCOME -> Welcome(model, ::addAccount)
            Step.CHOOSE -> ChooseAccount(model, providers) { choosing = false }
            Step.FORM -> model.accountForm?.let { AddAccountScreen(model, it, showProviders = false) { model.accountForm = null } }
            Step.NOTIFICATIONS -> Notifications(answer)
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

/** A step's page: content centred in a 440 dp column (392 inside its margins) and scrolling under large text, its buttons at the foot. On a tablet
 *  the column stops at 800 dp, so the buttons stay near the content. */
@Composable private fun StepPage(top: (@Composable () -> Unit)? = null, buttons: @Composable ColumnScope.() -> Unit, content: @Composable ColumnScope.() -> Unit) {
    Column(Modifier.fillMaxSize().systemBarsPadding()) {
        top?.invoke()
        Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
            Column(Modifier.fillMaxWidth().heightIn(max = 800.dp).fillMaxHeight()) {
                BoxWithConstraints(Modifier.weight(1f).fillMaxWidth()) {
                    Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).heightIn(min = maxHeight).padding(horizontal = 24.dp, vertical = 24.dp),
                        verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally) {
                        Column(Modifier.widthIn(max = 392.dp), horizontalAlignment = Alignment.CenterHorizontally, content = content)
                    }
                }
                Column(Modifier.widthIn(max = 440.dp).fillMaxWidth().align(Alignment.CenterHorizontally).padding(horizontal = 24.dp).padding(bottom = 16.dp),
                    verticalArrangement = Arrangement.spacedBy(8.dp), content = buttons)
            }
        }
    }
}

/** The site's display line: Hauora Bold with its tight tracking, growing with the font scale. */
@Composable private fun DisplayTitle(text: String, size: TextUnit, modifier: Modifier = Modifier) =
    Text(text, modifier.semantics { heading() }, fontFamily = DisplayFace, fontWeight = FontWeight.Bold, fontSize = size,
        letterSpacing = DisplayTracking.em, lineHeight = size * 1.1f, textAlign = TextAlign.Center)

@Composable private fun Lead(text: String, modifier: Modifier = Modifier, size: TextUnit = TextSize.body) =
    Text(text, modifier, color = lagom.ink2, fontSize = size, lineHeight = size * Leading, textAlign = TextAlign.Center)

/** The step's one way on: a house button across the column, 50 dp tall. */
@Composable private fun WideButton(text: String, @DrawableRes icon: Int, primary: Boolean = false, enabled: Boolean = true, onClick: () -> Unit) =
    HouseButton(text, icon, Modifier.fillMaxWidth().heightIn(min = 50.dp), primary = primary, enabled = enabled, onClick = onClick)

@Composable private fun Welcome(model: MailModel, addAccount: () -> Unit) {
    StepPage(buttons = {
        WideButton("Add account", R.drawable.ic_account_add, primary = true, enabled = model.core != null, onClick = addAccount)
        // The store could not open: the only way on is to try again.
        if (model.core == null && !model.loading) WideButton("Try again", R.drawable.ic_sync) { model.perform { model.setForeground(true) } }
    }) {
        AppIcon(88.dp)
        DisplayTitle("lagom mail", 40.sp, Modifier.padding(top = 24.dp))
        Lead("Calm email for a clear day.", Modifier.padding(top = 6.dp), TextSize.title)
        Column(Modifier.fillMaxWidth().padding(top = 40.dp), verticalArrangement = Arrangement.spacedBy(20.dp)) {
            Point(R.drawable.ic_done, "Done, Reply or Later", "Each message gets one answer. The list says how many are left.")
            Point(R.drawable.ic_sun, "Today in one sentence", "What is unread, what is due and what is next.")
            Point(R.drawable.ic_access, "Private by default", "No tracking and no ads. Your mail goes only to your own server.")
        }
    }
}

/** One reason to stay: a line icon in a quiet tile, a short title and one sentence. TalkBack reads it as one item. */
@Composable private fun Point(@DrawableRes icon: Int, title: String, detail: String) {
    val c = lagom
    Row(Modifier.fillMaxWidth().semantics(mergeDescendants = true) {}, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
        Box(Modifier.clip(RoundedCornerShape(Radius.control)).background(c.subtle).padding(10.dp)) { LagomIcon(icon, tint = c.accent, size = 20.dp) }
        Column(Modifier.weight(1f).padding(top = 1.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(title, style = MaterialTheme.typography.titleMedium)
            Text(detail, color = c.ink2, style = MaterialTheme.typography.bodyMedium)
        }
    }
}

@Composable private fun ChooseAccount(model: MailModel, providers: List<Pair<SignInProvider, String>>, back: () -> Unit) {
    val context = LocalContext.current
    StepPage(top = {
        TopBar(start = 4.dp) { LagomIconButton("Back", R.drawable.ic_chevron_left, onClick = back) }
    }, buttons = {}) {
        DisplayTitle("Choose your mail", 30.sp)
        Lead("lagom connects straight to your mail server, with nothing in between.", Modifier.padding(top = 8.dp))
        Column(Modifier.fillMaxWidth().padding(top = 32.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            providers.forEach { (provider, name) ->
                val logo = if (provider == SignInProvider.GOOGLE) R.drawable.logo_google else R.drawable.logo_microsoft
                Choice("Continue with $name", Color.White, { Image(painterResource(logo), null, Modifier.size(18.dp)) }) {
                    model.perform { openSignIn(context, requireNotNull(model.core).beginSignIn(provider).url) }
                }
            }
            Choice("iCloud, Fastmail and other (IMAP)", lagom.subtle, { LagomIcon(R.drawable.ic_mail, size = 18.dp) }) { model.accountForm = AccountForm() }
        }
        Row(Modifier.padding(top = 24.dp).semantics(mergeDescendants = true) {}, horizontalArrangement = Arrangement.spacedBy(6.dp),
            verticalAlignment = Alignment.CenterVertically) {
            LagomIcon(R.drawable.ic_access, size = 14.dp)
            Text("Passwords stay on this device.", color = lagom.ink2, style = MaterialTheme.typography.bodySmall)
        }
    }
}

/** One account kind, as desktop's "Continue with" buttons: the mark in a round tile, the name and a chevron. Google asks for
 *  its "G" on white, so provider marks sit on white in dark mode too. */
@Composable private fun Choice(title: String, tile: Color, mark: @Composable () -> Unit, onClick: () -> Unit) {
    val c = lagom
    val shape = RoundedCornerShape(Radius.panel)
    Row(Modifier.fillMaxWidth().heightIn(min = 56.dp).clip(shape).background(c.bg).border(1.dp, c.line, shape)
        .clickable(role = Role.Button, onClick = onClick).padding(horizontal = 16.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
        Box(Modifier.size(34.dp).clip(CircleShape).background(tile).border(1.dp, c.line, CircleShape), contentAlignment = Alignment.Center) { mark() }
        Text(title, Modifier.weight(1f), style = MaterialTheme.typography.bodyLarge.copy(fontSize = TextSize.body, lineHeight = TextSize.body * Leading,
            fontWeight = FontWeight.Medium))
        LagomIcon(R.drawable.ic_chevron_right, size = 14.dp)
    }
}

@Composable private fun Notifications(answer: (Boolean) -> Unit) {
    StepPage(buttons = {
        WideButton("Allow", R.drawable.ic_done, primary = true) { answer(true) }
        WideButton("Not now", R.drawable.ic_close) { answer(false) }
    }) {
        Box(Modifier.clip(RoundedCornerShape(Radius.card)).background(lagom.subtle).padding(22.dp)) {
            LagomIcon(R.drawable.ic_inbox, tint = lagom.accent, size = 30.dp)
        }
        DisplayTitle("New mail, when it arrives", 30.sp, Modifier.padding(top = 24.dp))
        Lead("lagom tells you about new mail in your Inbox. Nothing else.", Modifier.padding(top = 8.dp))
    }
}
