package app.lagommail.android

import androidx.activity.compose.BackHandler
import androidx.compose.animation.*
import androidx.compose.animation.core.EaseOut
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import app.lagommail.core.SignInProvider
import app.lagommail.core.signInAvailable

/** The first run, the same on iOS: Welcome, then the kind of account, then its form; once an account is in, one
 *  notifications step. Steps that do not apply are skipped. */
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

/** A step's page: content centred a little above the middle, its buttons at the foot. Under a bar the content starts at
 *  the top, as a form does. */
@Composable private fun StepPage(top: (@Composable () -> Unit)? = null, buttons: @Composable ColumnScope.() -> Unit, content: @Composable ColumnScope.() -> Unit) {
    Column(Modifier.fillMaxSize().systemBarsPadding()) {
        top?.invoke()
        Column(Modifier.weight(1f).fillMaxWidth().padding(horizontal = 32.dp).padding(top = if (top != null) 40.dp else 0.dp, bottom = 32.dp),
            verticalArrangement = if (top != null) Arrangement.Top else Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally, content = content)
        Column(Modifier.fillMaxWidth().widthIn(max = 480.dp).align(Alignment.CenterHorizontally).padding(horizontal = 24.dp).padding(bottom = 16.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp), content = buttons)
    }
}

@Composable private fun Welcome(model: MailModel, addAccount: () -> Unit) {
    StepPage(buttons = {
        HouseButton("Add account", R.drawable.ic_account_add, Modifier.fillMaxWidth().height(48.dp), primary = true, enabled = model.core != null, onClick = addAccount)
        // The store could not open: the only way on is to try again.
        if (model.core == null && !model.loading) HouseButton("Try again", modifier = Modifier.fillMaxWidth().height(48.dp)) {
            model.perform { model.setForeground(true) }
        }
    }) {
        AppIcon(88.dp)
        Text("lagom", Modifier.padding(top = 24.dp).semantics { heading() }, style = MaterialTheme.typography.displaySmall)
        Text("Mail, calendar and todo. Just right.", Modifier.padding(top = 8.dp), color = lagom.ink2,
            style = MaterialTheme.typography.bodyLarge, textAlign = TextAlign.Center)
    }
}

@Composable private fun ChooseAccount(model: MailModel, providers: List<Pair<SignInProvider, String>>, back: () -> Unit) {
    val context = LocalContext.current
    StepPage(top = {
        TopBar(start = 4.dp) {
            LagomIconButton("Back", R.drawable.ic_chevron_left, onClick = back)
            BarTitle("Add account")
        }
    }, buttons = {}) {
        Text("Choose your mail", Modifier.semantics { heading() }, style = MaterialTheme.typography.headlineSmall)
        Text("Google and Microsoft sign in in the browser. Anything else uses its mail server.", Modifier.padding(top = 6.dp, bottom = 24.dp),
            color = lagom.ink2, style = MaterialTheme.typography.bodyLarge, textAlign = TextAlign.Center)
        Column(Modifier.widthIn(max = 420.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            providers.forEach { (provider, name) ->
                Choice(name, R.drawable.ic_external) { model.perform { openSignIn(context, requireNotNull(model.core).beginSignIn(provider).url) } }
            }
            Choice("Other (IMAP)", R.drawable.ic_mail) { model.accountForm = AccountForm() }
        }
    }
}

/** One account kind: a filled row as tall as a list row, its icon, the name and a chevron. */
@Composable private fun Choice(title: String, icon: Int, onClick: () -> Unit) {
    val c = lagom
    Row(Modifier.fillMaxWidth().heightIn(min = 56.dp).clip(RoundedCornerShape(Radius.panel)).background(c.subtle)
        .clickable(role = Role.Button, onClick = onClick).padding(horizontal = 16.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        LagomIcon(icon, size = 20.dp)
        Text(title, Modifier.weight(1f), style = MaterialTheme.typography.bodyLarge)
        LagomIcon(R.drawable.ic_chevron_right, size = 16.dp)
    }
}

@Composable private fun Notifications(answer: (Boolean) -> Unit) {
    StepPage(buttons = {
        HouseButton("Allow", modifier = Modifier.fillMaxWidth().height(48.dp), primary = true) { answer(true) }
        HouseButton("Not now", modifier = Modifier.fillMaxWidth().height(48.dp)) { answer(false) }
    }) {
        LagomIcon(R.drawable.ic_inbox, size = 32.dp)
        Text("New mail, when it arrives", Modifier.padding(top = 16.dp).semantics { heading() }, style = MaterialTheme.typography.headlineSmall,
            textAlign = TextAlign.Center)
        Text("lagom tells you about new mail in your Inbox. Nothing else.", Modifier.padding(top = 6.dp), color = lagom.ink2,
            style = MaterialTheme.typography.bodyLarge, textAlign = TextAlign.Center)
    }
}
