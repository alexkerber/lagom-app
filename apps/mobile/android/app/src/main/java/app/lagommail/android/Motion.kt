package app.lagommail.android

import androidx.activity.BackEventCompat
import androidx.activity.compose.PredictiveBackHandler
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.EaseOut
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.MotionDurationScale
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

// docs/motion.md in Compose terms. Compose scales every animation by the system animator duration
// scale, so "Remove animations" ends them at once without code here.

/** Fades and colour: short and ease-out. */
internal val Fade = tween<Float>(150, easing = EaseOut)

/** Things that move and land: about 3 % overshoot, settled in about 200 ms. Never on opacity. */
internal fun <T> landing(threshold: T? = null) = spring(dampingRatio = 0.75f, stiffness = 700f, visibilityThreshold = threshold)

/** True once [active] has lasted 150 ms, so a quick load never flashes a spinner or a note. */
@Composable internal fun afterWait(active: Boolean): Boolean {
    var waited by remember { mutableStateOf(false) }
    LaunchedEffect(active) { if (active) delay(150); waited = active }
    return active && waited
}

/** True when a list change is small enough to animate. motion.md: more than five changes at once (a page loading) appear without motion. */
internal fun smallChange(before: List<Long>, after: List<Long>): Boolean {
    val old = before.toHashSet()
    val new = after.toHashSet()
    return after.count { it !in old } + before.count { it !in new } <= 5
}

internal class BackPreview {
    val progress = Animatable(0f)
    var edge by mutableIntStateOf(BackEventCompat.EDGE_LEFT)
}

/**
 * Predictive back: the screen shrinks toward the swipe while the finger moves. [onBack] runs on release and
 * returns true when the screen stays (for example a failed save), which springs it back, as a cancelled swipe does.
 */
@Composable internal fun rememberBackPreview(enabled: Boolean, onBack: suspend () -> Boolean): BackPreview {
    val preview = remember { BackPreview() }
    val scope = rememberCoroutineScope()
    val currentOnBack by rememberUpdatedState(onBack)
    PredictiveBackHandler(enabled) { events ->
        // With animations off nothing follows the finger; back still works.
        val still = currentCoroutineContext()[MotionDurationScale]?.scaleFactor == 0f
        try {
            events.collect { if (!still) { preview.edge = it.swipeEdge; preview.progress.snapTo(it.progress) } }
        } catch (cancelled: CancellationException) {
            scope.launch { preview.progress.animateTo(0f, landing()) }
            throw cancelled
        }
        if (currentOnBack()) preview.progress.animateTo(0f, landing())
    }
    return preview
}

/** Material's full-screen back preview: down to 90 %, a small shift away from the swiped edge, rounded corners. */
internal fun Modifier.backPreview(preview: BackPreview) = graphicsLayer {
    val p = preview.progress.value
    if (p == 0f) return@graphicsLayer
    val scale = 1f - 0.1f * p
    scaleX = scale; scaleY = scale
    val shift = (size.width / 20f - 8.dp.toPx()).coerceAtLeast(0f) * p
    translationX = if (preview.edge == BackEventCompat.EDGE_LEFT) shift else -shift
    shape = RoundedCornerShape(28.dp * p)
    clip = true
}
