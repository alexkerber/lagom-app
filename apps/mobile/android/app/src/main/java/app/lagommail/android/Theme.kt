package app.lagommail.android

import androidx.annotation.DrawableRes
import androidx.compose.animation.core.*
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.*
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/** Shared tokens from design/tokens.toml (Dusk accent), light and dark, as iOS `Theme`. */
@Immutable internal data class LagomColors(
    val bg: Color, val subtle: Color, val raised: Color, val ink: Color, val ink2: Color, val line: Color,
    /** `sel`: the neutral selection, under ink text. */
    val sel: Color,
    /** `sel_focus`: the accent as a fill under white text, the same in light and dark. */
    val selFocus: Color,
    /** `sel_icon`: the active page icon, lifted halfway to white in dark so it keeps 3:1 on `sel`. */
    val selIcon: Color,
    val accent: Color, val accentInk: Color,
    /** Primary buttons: solid Swedish blue under white text in both themes. */
    val accentFill: Color, val accentFillInk: Color,
    val done: Color, val later: Color, val danger: Color,
    /** The desktop sidebar's glass, as a solid fill: the page bar. */
    val sidebar: Color,
)

// BEGIN GENERATED TOKENS (scripts/tokens.py)
internal val LightColors = LagomColors(
    bg = Color(0xFFFFFFFF),
    subtle = Color(0xFFF5F5F3),
    raised = Color(0xFFFFFFFF),
    ink = Color(0xFF111110),
    ink2 = Color(0xFF5F5F5A),
    line = Color(0xFFE6E6E2),
    sel = Color(0xFFDCDCDC),
    selFocus = Color(0xFF2A5BB8),
    selIcon = Color(0xFF2A5BB8),
    accent = Color(0xFF2A5BB8),
    accentInk = Color(0xFFFFFFFF),
    accentFill = Color(0xFF2A5BB8),
    accentFillInk = Color(0xFFFFFFFF),
    done = Color(0xFF1E6B4E),
    later = Color(0xFF94640A),
    danger = Color(0xFFC62828),
    sidebar = Color(0xFFF3F2EF),
)

internal val DarkColors = LagomColors(
    bg = Color(0xFF1E1E1E),
    subtle = Color(0xFF262626),
    raised = Color(0xFF2C2C2C),
    ink = Color(0xFFF2F2EE),
    ink2 = Color(0xFF9B9B95),
    line = Color(0xFF3A3A3A),
    sel = Color(0xFF464646),
    selFocus = Color(0xFF2A5BB8),
    selIcon = Color(0xFF94ADDC),
    accent = Color(0xFF8AB0F5),
    accentInk = Color(0xFF1E1E1E),
    accentFill = Color(0xFF2F63CC),
    accentFillInk = Color(0xFFFFFFFF),
    done = Color(0xFF4CC38A),
    later = Color(0xFFF5C22E),
    danger = Color(0xFFF87171),
    sidebar = Color(0xFF2A2A2A),
)

internal object TextSize {
    val small = 13.sp
    val snippet = 14.sp
    val base = 15.sp
    val body = 16.sp
    val title = 17.sp
    val subject = 23.sp
    val display = 30.sp
    val hero = 56.sp
}

internal val TextFace = FontFamily(Font(R.font.hauora_regular, FontWeight.Normal), Font(R.font.hauora_medium, FontWeight.Medium), Font(R.font.hauora_semibold, FontWeight.SemiBold), Font(R.font.hauora_bold, FontWeight.Bold))
internal val DisplayFace = FontFamily(Font(R.font.hauora_bold, FontWeight.Bold))
internal val LabelFace = FontFamily(Font(R.font.geist_mono_medium, FontWeight.Medium))
internal const val DisplayTracking = -0.045f

internal object Radius {
    val key = 4.dp
    val control = 8.dp
    val attachment = 10.dp
    val panel = 12.dp
    val card = 16.dp
}

internal const val Leading = 1.3f
internal const val ReadingLeading = 1.45f

internal object MotionTokens {
    const val calm = 150
    const val fade = 150
    const val empty = 300
    const val pulse = 1600
    const val damping = 0.75f
    const val stiffness = 700f
}
// END GENERATED TOKENS

internal val LocalLagomColors = staticCompositionLocalOf { LightColors }
internal val lagom: LagomColors @Composable @ReadOnlyComposable get() = LocalLagomColors.current

/** The three accents in turn, as desktop `account_accent`: account marks and To-do list dots. */
@Composable @ReadOnlyComposable internal fun accountAccent(index: Int): Color = listOf(lagom.accent, lagom.done, lagom.later)[index % 3]

/** Every bar on every page is this tall, so moving between pages never shifts the title. */
internal val BarHeight = 56.dp

// Hauora for all text (Alex 2026-11-20: "samma typsnitt överallt"), with Android's existing line-height override.
private fun style(size: TextUnit, weight: FontWeight = FontWeight.Normal) =
    TextStyle(fontFamily = TextFace, fontSize = size, lineHeight = size * Leading, fontWeight = weight, letterSpacing = 0.sp)

private val LagomType = Typography(
    displayLarge = style(TextSize.display, FontWeight.SemiBold), displayMedium = style(TextSize.display, FontWeight.SemiBold),
    displaySmall = style(TextSize.display, FontWeight.SemiBold),
    headlineLarge = style(TextSize.subject, FontWeight.SemiBold), headlineMedium = style(TextSize.subject, FontWeight.SemiBold),
    headlineSmall = style(TextSize.subject, FontWeight.SemiBold),
    titleLarge = style(TextSize.title, FontWeight.SemiBold), titleMedium = style(TextSize.base, FontWeight.SemiBold),
    titleSmall = style(TextSize.snippet, FontWeight.Medium),
    bodyLarge = style(TextSize.base), bodyMedium = style(TextSize.snippet), bodySmall = style(TextSize.small),
    labelLarge = style(TextSize.base, FontWeight.Medium), labelMedium = style(TextSize.small, FontWeight.Medium), labelSmall = style(TextSize.small),
)

/** Every Material slot from the tokens, so no stock purple, tonal tint or dynamic colour shows anywhere. */
private fun scheme(c: LagomColors, dark: Boolean): ColorScheme {
    val other = if (dark) LightColors else DarkColors
    val base = if (dark) darkColorScheme() else lightColorScheme()
    return base.copy(
        primary = c.accent, onPrimary = c.accentInk, primaryContainer = c.sel, onPrimaryContainer = c.ink,
        inversePrimary = other.accent,
        secondary = c.accent, onSecondary = c.accentInk, secondaryContainer = c.sel, onSecondaryContainer = c.ink,
        tertiary = c.done, onTertiary = c.accentInk, tertiaryContainer = c.sel, onTertiaryContainer = c.ink,
        background = c.bg, onBackground = c.ink, surface = c.bg, onSurface = c.ink,
        surfaceVariant = c.subtle, onSurfaceVariant = c.ink2, surfaceTint = c.bg,
        inverseSurface = c.ink, inverseOnSurface = c.bg,
        error = c.danger, onError = c.accentInk, errorContainer = c.sel, onErrorContainer = c.danger,
        outline = c.line, outlineVariant = c.line, scrim = Color.Black,
        surfaceBright = c.raised, surfaceDim = c.subtle,
        surfaceContainerLowest = c.bg, surfaceContainerLow = c.subtle, surfaceContainer = c.raised,
        surfaceContainerHigh = c.raised, surfaceContainerHighest = c.raised,
    )
}

@Composable internal fun LagomTheme(dark: Boolean, content: @Composable () -> Unit) {
    val colors = if (dark) DarkColors else LightColors
    CompositionLocalProvider(LocalLagomColors provides colors) {
        MaterialTheme(colorScheme = scheme(colors, dark), typography = LagomType, shapes = Shapes(
            extraSmall = RoundedCornerShape(Radius.key), small = RoundedCornerShape(Radius.control), medium = RoundedCornerShape(Radius.panel),
            large = RoundedCornerShape(Radius.card), extraLarge = RoundedCornerShape(Radius.card),
        )) {
            // Text outside a Surface still gets ink.
            CompositionLocalProvider(LocalContentColor provides colors.ink, content = content)
        }
    }
}

/** A desktop line icon (`apps/desktop/assets/icons`, converted by scripts/android-icons.py). Without a label it is decoration. */
@Composable internal fun LagomIcon(@DrawableRes id: Int, modifier: Modifier = Modifier, tint: Color = lagom.ink2, size: Dp = 18.dp, label: String? = null) =
    Icon(painterResource(id), label, modifier.size(size), tint = tint)

/** A push button, desktop `button`: bordered, or filled with the accent when `primary`. 36 dp drawn, 48 dp to touch. */
@Composable internal fun HouseButton(text: String, @DrawableRes icon: Int? = null, modifier: Modifier = Modifier, primary: Boolean = false,
    iconOnly: Boolean = false, enabled: Boolean = true, danger: Boolean = false, onClick: () -> Unit) {
    val c = lagom
    val ink = when { primary -> c.accentFillInk; danger -> c.danger; else -> c.ink }
    Box(modifier, contentAlignment = Alignment.Center,
        // A full-width button passes its width on to the drawn shape.
        propagateMinConstraints = true) {
        Row(Modifier.minimumInteractiveComponentSize().heightIn(min = 36.dp).clip(RoundedCornerShape(Radius.control))
            // Disabled dims fill and label alike. A layer in both states: Modifier.alpha drops its layer at 1f, and that
            // chain change left the fill undrawn when a disabled button turned enabled (Welcome, emulator-5604, 2026-11-20).
            .graphicsLayer { alpha = if (enabled) 1f else 0.5f }
            // No outline, as desktop since #168: a quiet fill marks the button.
            .background(if (primary) c.accentFill else c.subtle)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .semantics { if (iconOnly) contentDescription = text }
            .padding(horizontal = if (iconOnly) 9.dp else 12.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp, Alignment.CenterHorizontally)) {
            icon?.let { LagomIcon(it, tint = if (primary) c.accentFillInk else if (danger) c.danger else c.ink2) }
            if (!iconOnly) Text(text, color = ink, style = MaterialTheme.typography.labelLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}

/** A borderless glyph, desktop `icon_button`: 36 dp drawn, 48 dp to touch. */
@Composable internal fun LagomIconButton(label: String, @DrawableRes icon: Int, modifier: Modifier = Modifier, enabled: Boolean = true,
    tint: Color = lagom.ink2, onClick: () -> Unit) {
    Box(modifier.size(48.dp), contentAlignment = Alignment.Center) {
        Box(Modifier.size(40.dp).clip(RoundedCornerShape(Radius.control)).clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .semantics { contentDescription = label }.alpha(if (enabled) 1f else 0.4f), contentAlignment = Alignment.Center) {
            LagomIcon(icon, tint = tint, size = 20.dp)
        }
    }
}

/** The bar at the top of every page: one height, nothing that grows or collapses. No line under it, as desktop since #168. */
@Composable internal fun TopBar(modifier: Modifier = Modifier, start: Dp = 16.dp, end: Dp = 4.dp, content: @Composable RowScope.() -> Unit) {
    Row(modifier.fillMaxWidth().height(BarHeight).background(lagom.bg)
        .padding(start = start, end = end),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp), content = content)
}

/** A 1 dp `line` along the bottom, or the top, as desktop's bar borders. */
internal fun Modifier.drawHairline(color: Color, top: Boolean = false) = drawBehind {
    drawRect(color, topLeft = Offset(0f, if (top) 0f else size.height - 1.dp.toPx()), size = Size(size.width, 1.dp.toPx()))
}

/** A page title as on desktop and iOS: semibold at the title size. */
@Composable internal fun BarTitle(title: String, modifier: Modifier = Modifier) =
    Text(title, modifier.semantics { heading() }, style = MaterialTheme.typography.titleLarge.copy(fontFamily = DisplayFace, fontWeight = FontWeight.Bold), maxLines = 1, overflow = TextOverflow.Ellipsis)

/** The desktop's empty pages: a large line icon over a title and one quiet line, and an optional action. Fades in after
 *  150 ms, so a quick load never flashes it (motion.md). */
@Composable internal fun EmptyState(@DrawableRes icon: Int, title: String, modifier: Modifier = Modifier, detail: String? = null,
    action: (@Composable () -> Unit)? = null) {
    val shown = remember { Animatable(0f) }
    LaunchedEffect(Unit) { shown.animateTo(1f, tween(MotionTokens.empty, delayMillis = MotionTokens.calm, easing = EaseOut)) }
    Column(modifier.fillMaxSize().graphicsLayer { alpha = shown.value }.padding(horizontal = 32.dp).padding(bottom = 64.dp),
        verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally) {
        LagomIcon(icon, Modifier.padding(bottom = 16.dp), size = 32.dp)
        Text(title, Modifier.semantics { heading() }, style = MaterialTheme.typography.headlineSmall.copy(fontFamily = DisplayFace, fontWeight = FontWeight.Bold), textAlign = TextAlign.Center)
        detail?.let { Text(it, Modifier.padding(top = 6.dp), color = lagom.ink2, style = MaterialTheme.typography.bodyLarge, textAlign = TextAlign.Center) }
        action?.let { Box(Modifier.padding(top = 20.dp)) { it() } }
    }
}

/** Placeholder rows while a list's first read runs, shaped like message rows and fading down the list (desktop
 *  `loading_rows`). They wait 150 ms, then pulse slowly; with animations off they stay still. */
@Composable internal fun LoadingRows(modifier: Modifier = Modifier) {
    if (!afterWait(true)) return
    val pulse = rememberInfiniteTransition(label = "loading")
    val phase by pulse.animateFloat(0f, 1f, infiniteRepeatable(tween(MotionTokens.pulse, easing = LinearEasing)), label = "pulse")
    val shown = remember { Animatable(0f) }
    LaunchedEffect(Unit) { shown.animateTo(1f, Fade) }
    val bar = lagom.line
    Column(modifier.fillMaxWidth().clearAndSetSemantics { contentDescription = "Loading mail" }
        .graphicsLayer { alpha = shown.value * (0.7f + 0.3f * kotlin.math.cos(2 * Math.PI.toFloat() * phase)) }) {
        listOf(0.42f, 0.3f, 0.5f, 0.36f, 0.46f).forEachIndexed { i, w ->
            Column(Modifier.fillMaxWidth().height(RowHeight).alpha(1f - i * 0.18f).padding(start = 33.dp, end = 16.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterVertically)) {
                listOf(w to 10.dp, (w + 0.3f) to 10.dp, 0.9f to 8.dp, 0.62f to 8.dp).forEach { (width, height) ->
                    Box(Modifier.fillMaxWidth(width).height(height).background(bar, RoundedCornerShape(Radius.key)))
                }
            }
        }
    }
}

/** A message row's height in the list, so loading rows line up with the rows that replace them. */
internal val RowHeight = 92.dp
