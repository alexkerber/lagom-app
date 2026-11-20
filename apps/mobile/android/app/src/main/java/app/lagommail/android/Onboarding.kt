package app.lagommail.android

import androidx.activity.compose.BackHandler
import androidx.annotation.DrawableRes
import androidx.compose.animation.*
import androidx.compose.animation.core.EaseOut
import androidx.compose.animation.core.animateFloatAsState
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
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.sp
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.isSpecified
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInWindow
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.em
import androidx.compose.ui.util.lerp
import app.lagommail.core.SignInProvider
import app.lagommail.core.signInAvailable
import kotlinx.coroutines.delay

/** The first run, the same on iOS: Welcome, then the kind of account, then its form; once an account is in, one
 *  notifications step. The brand of DESIGN_PROMPT.md: paper, ink and one accent; flat, hairlines, small radii; the
 *  dictionary entry as hero. Spacing is on an 8 dp grid. Copy matches iOS word for word. */
private enum class Step { WELCOME, CHOOSE, FORM, NOTIFICATIONS }

/** The splash draws the adaptive icon in a 160 dp circle, centred in the window. */
internal val SplashIcon = 160.dp

@Composable internal fun Onboarding(model: MailModel, askNotifications: Boolean, demo: Boolean, answer: (allow: Boolean) -> Unit) {
    var choosing by rememberSaveable { mutableStateOf(false) }
    // Welcome's icon settles once; Back to Welcome does not replay the splash hand-over.
    var settled by rememberSaveable { mutableStateOf(false) }
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
    // motion.md: forward steps arrive from the end, back from the start; 250 ms ease-out with a fade, no bounce. "Remove
    // animations" in the system settings leaves only the change.
    AnimatedContent(step, Modifier.fillMaxSize(), label = "onboarding", transitionSpec = {
        val forward = targetState.ordinal > initialState.ordinal
        val move = tween<IntOffset>(250, easing = EaseOut)
        (fadeIn(tween(250, easing = EaseOut)) + slideInHorizontally(move) { if (forward) it / 8 else -it / 8 }) togetherWith
            (fadeOut(tween(150, easing = EaseOut)) + slideOutHorizontally(move) { if (forward) -it / 8 else it / 8 })
    }) { shown ->
        when (shown) {
            Step.WELCOME -> Welcome(model, settled, { settled = true }, ::addAccount)
            Step.CHOOSE -> ChooseAccount(model, providers) { choosing = false }
            Step.FORM -> model.accountForm?.let { AddAccountScreen(model, it, showProviders = false) { model.accountForm = null } }
            Step.NOTIFICATIONS -> Notifications(answer)
        }
    }
}

/** The app icon, as the launcher draws it: the adaptive layers cropped to their visible middle. [corner] is a share of
 *  [size]: 0.24 for the launcher's rounded square, 0.5 for the splash circle. */
@Composable internal fun AppIcon(size: Dp, modifier: Modifier = Modifier, corner: Float = 0.24f) {
    Box(modifier.size(size).clip(RoundedCornerShape(size * corner)), contentAlignment = Alignment.Center) {
        // Adaptive icons show the middle 72 of their 108 units.
        Image(painterResource(R.drawable.icon_background), null, Modifier.requiredSize(size * 1.5f))
        Image(painterResource(R.drawable.icon_foreground), null, Modifier.requiredSize(size * 1.5f))
    }
}

/** A step's page: content in a 440 dp column (392 inside its margins), scrolling under large text, its buttons at the foot.
 *  On a tablet the column stops at 800 dp, so the buttons stay near the content. */
@Composable private fun StepPage(modifier: Modifier = Modifier, top: (@Composable () -> Unit)? = null, centred: Boolean = true,
    buttons: @Composable ColumnScope.() -> Unit, content: @Composable ColumnScope.() -> Unit) {
    Column(modifier.fillMaxSize().systemBarsPadding()) {
        // The same 56 dp bar on every step, so the column starts at one height.
        Box(Modifier.fillMaxWidth().height(56.dp).padding(horizontal = 4.dp), contentAlignment = Alignment.CenterStart) { top?.invoke() }
        Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
            Column(Modifier.fillMaxWidth().heightIn(max = 800.dp).fillMaxHeight()) {
                BoxWithConstraints(Modifier.weight(1f).fillMaxWidth()) {
                    Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).heightIn(min = maxHeight).padding(horizontal = 24.dp).padding(bottom = 32.dp),
                        verticalArrangement = if (centred) Arrangement.Center else Arrangement.Top, horizontalAlignment = Alignment.CenterHorizontally) {
                        Column(Modifier.widthIn(max = 392.dp).fillMaxWidth(), content = content)
                    }
                }
                Column(Modifier.widthIn(max = 440.dp).fillMaxWidth().align(Alignment.CenterHorizontally).padding(horizontal = 24.dp).padding(bottom = 16.dp),
                    verticalArrangement = Arrangement.spacedBy(8.dp), content = buttons)
            }
        }
    }
}

/** The site's display face: Hauora Bold with its tight tracking, growing with the font scale. */
@Composable private fun DisplayTitle(text: String, size: TextUnit, modifier: Modifier = Modifier) =
    Text(text, modifier.semantics { heading() }, color = lagom.ink, fontFamily = DisplayFace, fontWeight = FontWeight.Bold, fontSize = size,
        letterSpacing = DisplayTracking.em, lineHeight = size * 1.15f)

@Composable private fun Lead(text: String, modifier: Modifier = Modifier, size: TextUnit = TextSize.body) =
    Text(text, modifier, color = lagom.ink2, fontSize = size, lineHeight = size * Leading)

/** The step's one way on: a house button across the column, 48 dp tall. */
@Composable private fun WideButton(text: String, @DrawableRes icon: Int, primary: Boolean = false, enabled: Boolean = true, onClick: () -> Unit) =
    HouseButton(text, icon, Modifier.fillMaxWidth().heightIn(min = 48.dp), primary = primary, enabled = enabled, onClick = onClick)

/** Welcome, as on Mac and iOS: one centred group (icon, wordmark, dictionary line, definition, button, caption) centred on
 *  the screen, scrolling under large text. The wordmark is the one heavy element. */
@Composable private fun Welcome(model: MailModel, settled: Boolean, settle: () -> Unit, addAccount: () -> Unit) {
    val c = lagom
    // The icon starts where the splash drew it (160 dp circle, window centre) and settles into place.
    LaunchedEffect(Unit) { if (!settled) { delay(300); settle() } }
    val t by animateFloatAsState(if (settled) 1f else 0f, tween(450, easing = EaseOut), label = "settle")
    var centre by remember { mutableStateOf(Offset.Unspecified) }
    var mark by remember { mutableStateOf(Offset.Unspecified) }
    val icon = 96.dp
    BoxWithConstraints(Modifier.fillMaxSize().onGloballyPositioned { centre = it.positionInWindow() + Offset(it.size.width / 2f, it.size.height / 2f) }
        .systemBarsPadding()) {
        Box(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).heightIn(min = maxHeight).padding(horizontal = 24.dp, vertical = 16.dp),
            contentAlignment = Alignment.Center) {
            Column(horizontalAlignment = Alignment.CenterHorizontally) {
                val measured = centre.isSpecified && mark.isSpecified
                AppIcon(icon, Modifier.onGloballyPositioned { mark = it.positionInWindow() + Offset(it.size.width / 2f, it.size.height / 2f) }
                    .graphicsLayer {
                        if (measured) {
                            translationX = (centre.x - mark.x) * (1 - t); translationY = (centre.y - mark.y) * (1 - t)
                            scaleX = lerp(SplashIcon / icon, 1f, t); scaleY = scaleX
                        }
                        alpha = if (measured) 1f else 0f
                    }, corner = lerp(0.5f, 0.24f, t))
                // Hidden until the icon settles, so it takes no tap or TalkBack focus before it shows.
                Column(Modifier.graphicsLayer { alpha = t }.then(if (settled) Modifier else Modifier.clearAndSetSemantics {}),
                    horizontalAlignment = Alignment.CenterHorizontally) {
                    DisplayTitle("lagom", TextSize.hero, Modifier.padding(top = 24.dp))
                    Text(buildAnnotatedString {
                        append("/ˈlɑːɡɔm/ ")
                        withStyle(SpanStyle(color = c.ink2)) { append("adv., adj. Swedish.") }
                    }, Modifier.padding(top = 8.dp).clearAndSetSemantics { contentDescription = "Pronounced LAH-gom. Adverb, adjective. Swedish." },
                        color = c.ink, fontFamily = TextFace, fontWeight = FontWeight.Normal, fontSize = 15.sp, textAlign = TextAlign.Center)
                    // One line where it fits; else one sentence a line, so no width leaves a word alone.
                    var stacked by remember { mutableStateOf(false) }
                    Text(if (stacked) "Not too much.\nNot too little.\nJust right." else "Not too much. Not too little. Just right.",
                        Modifier.padding(top = 20.dp), color = c.ink, fontFamily = TextFace, fontWeight = FontWeight.Normal, fontSize = 17.sp,
                        lineHeight = 17.sp * Leading, textAlign = TextAlign.Center, softWrap = stacked,
                        onTextLayout = { if (!stacked && it.hasVisualOverflow) stacked = true })
                    Column(Modifier.padding(top = 32.dp).width(280.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        HouseButton("Add account", modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp), primary = true,
                            enabled = model.core != null && settled, onClick = addAccount)
                        // The store could not open: the only way on is to try again.
                        if (model.core == null && !model.loading) WideButton("Try again", R.drawable.ic_sync, enabled = settled) { model.perform { model.setForeground(true) } }
                    }
                    Text("No tracking. No lagom servers.", Modifier.padding(top = 12.dp), color = c.ink2, fontSize = TextSize.small, textAlign = TextAlign.Center)
                }
            }
        }
    }
}

@Composable private fun ChooseAccount(model: MailModel, providers: List<Pair<SignInProvider, String>>, back: () -> Unit) {
    val context = LocalContext.current
    StepPage(top = { LagomIconButton("Back", R.drawable.ic_chevron_left, onClick = back) }, centred = false, buttons = {}) {
        DisplayTitle("Choose your mail", TextSize.display, Modifier.padding(top = 8.dp))
        if (providers.isNotEmpty()) {
            // Hidden until this build has the provider's client ID, as in Add account. Google asks for its full button text
            // and its "G" on white.
            Rows(Modifier.padding(top = 24.dp)) {
                providers.forEachIndexed { index, (provider, name) ->
                    if (index > 0) Hairline()
                    val logo = if (provider == SignInProvider.GOOGLE) R.drawable.logo_google else R.drawable.logo_microsoft
                    Choice("Continue with $name", {
                        Box(Modifier.size(32.dp).clip(CircleShape).background(Color.White).border(1.dp, lagom.line, CircleShape), contentAlignment = Alignment.Center) {
                            Image(painterResource(logo), null, Modifier.size(16.dp))
                        }
                    }) { model.perform { openSignIn(context, requireNotNull(model.core).beginSignIn(provider).url) } }
                }
            }
        }
        MonoLabel("Other mail", Modifier.padding(top = 32.dp, bottom = 8.dp))
        // The address decides the servers, so every mail app opens the same form.
        Rows {
            listOf("iCloud", "Fastmail", "Yahoo", "Other (IMAP)")
                .forEachIndexed { index, name ->
                    if (index > 0) Hairline()
                    Choice(name, { LagomIcon(R.drawable.ic_mail, size = 18.dp) }) { model.accountForm = AccountForm() }
                }
        }
        Row(Modifier.padding(top = 16.dp).semantics(mergeDescendants = true) {}, horizontalArrangement = Arrangement.spacedBy(8.dp),
            verticalAlignment = Alignment.CenterVertically) {
            LagomIcon(R.drawable.ic_access, size = 14.dp)
            Text("Passwords stay on this device.", color = lagom.ink2, style = MaterialTheme.typography.bodySmall)
        }
    }
}

/** A list of rows in one hairline frame, like a shopping list. */
@Composable internal fun Rows(modifier: Modifier = Modifier, content: @Composable ColumnScope.() -> Unit) {
    val shape = RoundedCornerShape(Radius.control)
    Column(modifier.fillMaxWidth().clip(shape).border(1.dp, lagom.line, shape), content = content)
}

/** Between rows, from the text's edge: 16 inset + 32 mark + 16 gap; 16 where rows have no mark. */
@Composable internal fun Hairline(inset: Int = 64) = HorizontalDivider(Modifier.padding(start = inset.dp), thickness = 1.dp, color = lagom.line)

/** One account kind: its mark, the name and a chevron. */
@Composable private fun Choice(title: String, mark: @Composable () -> Unit, onClick: () -> Unit) {
    Row(Modifier.fillMaxWidth().heightIn(min = 56.dp).clickable(role = Role.Button, onClick = onClick).padding(horizontal = 16.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(16.dp)) {
        Box(Modifier.width(32.dp), contentAlignment = Alignment.Center) { mark() }
        Text(title, Modifier.weight(1f), color = lagom.ink, fontSize = TextSize.body, lineHeight = TextSize.body * Leading, fontWeight = FontWeight.Medium)
        LagomIcon(R.drawable.ic_chevron_right, size = 14.dp)
    }
}

@Composable private fun Notifications(answer: (Boolean) -> Unit) {
    StepPage(buttons = {
        WideButton("Allow", R.drawable.ic_done, primary = true) { answer(true) }
        WideButton("Not now", R.drawable.ic_close) { answer(false) }
    }) {
        LagomIcon(R.drawable.ic_inbox, tint = lagom.accent, size = 32.dp)
        DisplayTitle("Notifications", TextSize.display, Modifier.padding(top = 24.dp))
        Lead("lagom tells you when new mail arrives in your Inbox. Nothing else.", Modifier.padding(top = 8.dp))
    }
}
