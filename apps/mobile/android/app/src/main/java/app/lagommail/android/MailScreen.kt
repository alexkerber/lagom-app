package app.lagommail.android

import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.Intent
import android.webkit.MimeTypeMap
import androidx.core.content.FileProvider
import androidx.compose.animation.*
import androidx.compose.animation.core.*
import androidx.compose.foundation.background
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.addPathNodes
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.semantics.*
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import app.lagommail.core.*
import kotlinx.coroutines.launch
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle
import kotlin.math.abs

@Composable internal fun MailScreen(model: MailModel, twoPane: Boolean) {
    var retrying by remember { mutableStateOf<SendFailure?>(null) }
    var removing by remember { mutableStateOf<MailAccount?>(null) }
    var signing by remember { mutableStateOf<MailAccount?>(null) }
    var naming by remember { mutableStateOf<MailAccount?>(null) }
    // Compose and Add account cover the mail screen instead of replacing it, so the list keeps its
    // place. The form or draft rides along so a leaving screen keeps its content while it animates out.
    val overlay: Any? = model.accountForm ?: model.draft
    val rise = with(LocalDensity.current) { 12.dp.roundToPx() }
    Box(Modifier.fillMaxSize()) {
        Box(if (overlay != null) Modifier.clearAndSetSemantics {} else Modifier) {
            MailHome(model, twoPane, onRemove = { removing = it }, onSign = { signing = it }, onName = { naming = it }, onRetry = { retrying = it })
        }
        AnimatedContent(overlay, Modifier.fillMaxSize(), contentKey = { it?.let { screen -> screen::class } }, label = "screen", transitionSpec = {
            when {
                // motion.md: compose rises 12 dp into place on the spring and fades in; Undo after Send plays the same arrival.
                targetState is ComposeDraft -> (fadeIn(tween(200, easing = EaseOut)) +
                    slideInVertically(landing(IntOffset.VisibilityThreshold)) { rise }) togetherWith fadeOut(Fade)
                // Send or close: compose slides up and out with an ease-out.
                initialState is ComposeDraft -> fadeIn(Fade) togetherWith (fadeOut(tween(200, easing = EaseOut)) +
                    slideOutVertically(tween(200, easing = EaseOut)) { -2 * rise })
                else -> fadeIn(Fade) togetherWith fadeOut(Fade)
            } using null
        }) { screen ->
            when (screen) {
                is AccountForm -> AddAccountScreen(model, screen) { model.accountForm = null }
                is ComposeDraft -> ComposeScreen(model, screen)
            }
        }
    }
    model.error?.let { error ->
        AlertDialog(onDismissRequest = { model.error = null }, title = { Text("Mail") }, text = { Text(error) },
            confirmButton = { TextButton(onClick = { model.error = null }) { Text("OK") } })
    }
    removing?.let { account ->
        AlertDialog(onDismissRequest = { removing = null }, title = { Text("Remove account?") }, text = { Text("Remove ${account.email} and its local mail?") },
            confirmButton = { TextButton(onClick = { removing = null; model.perform { model.remove(account) } }) { Text("Remove") } },
            dismissButton = { TextButton(onClick = { removing = null }) { Text("Cancel") } })
    }
    signing?.let { account -> SignatureDialog(model, account) { signing = null } }
    naming?.let { account -> NameDialog(model, account) { naming = null } }
    retrying?.let { failure ->
        AlertDialog(onDismissRequest = { retrying = null }, title = { Text("Send again?") },
            text = { Text("Check Sent first if delivery was uncertain. This can send a duplicate.") },
            confirmButton = { TextButton(onClick = { retrying = null; model.perform { model.core?.retrySend(failure.id); model.sync() } }) { Text("Send again") } },
            dismissButton = { TextButton(onClick = { retrying = null }) { Text("Cancel") } })
    }
}

internal fun Mailbox.label(): String = when (this) {
    Mailbox.INBOX -> "Inbox"; Mailbox.LATER -> "Later"; Mailbox.DONE -> "Done"; Mailbox.SENT -> "Sent"; Mailbox.TRASH -> "Trash"
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable private fun MailHome(model: MailModel, twoPane: Boolean, onRemove: (MailAccount) -> Unit, onSign: (MailAccount) -> Unit,
    onName: (MailAccount) -> Unit, onRetry: (SendFailure) -> Unit) {
    var menu by remember { mutableStateOf(false) }
    Scaffold(topBar = {
        TopAppBar(title = {
            Text(if (model.showingDrafts) "Drafts" else if (model.accounts.isEmpty()) "lagom" else model.folder?.name ?: model.mailbox.label(),
                maxLines = 1, overflow = TextOverflow.Ellipsis)
        }, navigationIcon = {
            Box {
                TextButton(onClick = { menu = true }) { Text("Mailboxes") }
                DropdownMenu(menu, { menu = false }) {
                    Mailbox.entries.forEach { box ->
                        DropdownMenuItem(text = { Text(box.label()) }, onClick = {
                            menu = false; model.perform { model.changeScope(model.accountId, box) }
                        })
                    }
                    DropdownMenuItem(text = { Text("Drafts") }, onClick = {
                        menu = false; model.perform { model.showDrafts() }
                    })
                    // Desktop's folder group: the server's own folders, with the account when there are several.
                    val folders = model.folders.filter { model.accountId == null || it.account == model.accountId }
                    if (folders.isNotEmpty()) HorizontalDivider()
                    folders.forEach { folder ->
                        val account = model.accounts.find { it.id == folder.account }?.takeIf { model.accounts.size > 1 }
                        DropdownMenuItem(text = { Text(account?.let { "${folder.path} · ${it.displayName}" } ?: folder.path) }, onClick = {
                            menu = false; model.perform { model.openFolder(folder) }
                        })
                    }
                    HorizontalDivider()
                    DropdownMenuItem(text = { Text("All accounts") }, onClick = {
                        menu = false; model.perform { model.changeScope(null, model.mailbox) }
                    })
                    model.accounts.forEach { account ->
                        DropdownMenuItem(text = { Text(account.email) }, onClick = {
                            menu = false; model.perform { model.changeScope(account.id, model.mailbox) }
                        })
                    }
                    DropdownMenuItem(text = { Text("Add account") }, enabled = model.core != null, onClick = { menu = false; model.accountForm = AccountForm() })
                    model.accounts.forEach { account ->
                        DropdownMenuItem(text = { Text("Your name for ${account.email}") }, onClick = { menu = false; onName(account) })
                        DropdownMenuItem(text = { Text("Signature for ${account.email}") }, onClick = { menu = false; onSign(account) })
                        DropdownMenuItem(text = { Text("Remove ${account.email}") }, onClick = { menu = false; onRemove(account) })
                    }
                }
            }
        }, actions = {
            TextButton(enabled = model.accounts.isNotEmpty(), onClick = {
                val account = model.accountId ?: model.accounts.first().id
                model.perform { model.newMessage(account) }
            }) { Text("Compose") }
        })
    }, snackbarHost = { Notice(model) }) { padding ->
        Column(Modifier.fillMaxSize().padding(padding)) {
            if (afterWait(model.loading)) LinearProgressIndicator(Modifier.fillMaxWidth())
            if (model.accounts.isEmpty() && !model.loading) {
                Column(Modifier.fillMaxSize().padding(24.dp), verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally) {
                    Text("Welcome to lagom", style = MaterialTheme.typography.headlineMedium)
                    Text("Add an account to get started.")
                    Button(enabled = model.core != null, onClick = { model.accountForm = AccountForm() }) { Text("Add account") }
                    if (model.core == null) TextButton(onClick = { model.perform { model.setForeground(true) } }) { Text("Try again") }
                }
            } else if (model.showingDrafts) {
                LazyColumn(Modifier.weight(1f)) {
                    val drafts = model.drafts.filter { model.accountId == null || it.account == model.accountId }
                    if (drafts.isEmpty()) item(key = "empty") { Text("No drafts", rowMotion(true).padding(24.dp)) }
                    items(drafts, key = { it.id }) { draft ->
                        Column(rowMotion(true)) {
                            ListItem(headlineContent = { Text(draft.subject.ifEmpty { "(No subject)" }, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                                supportingContent = { Column {
                                    Text(draft.to)
                                    Text(model.accounts.find { it.id == draft.account }?.email.orEmpty())
                                    if (draft.updatedUnix > 0) Text(DateTimeFormatter.ofLocalizedDateTime(FormatStyle.SHORT)
                                        .withZone(ZoneId.systemDefault()).format(java.time.Instant.ofEpochSecond(draft.updatedUnix)))
                                } }, modifier = Modifier.heightIn(min = 48.dp).clickable(onClickLabel = "Edit draft") { model.perform { model.openDraft(draft) } })
                            HorizontalDivider()
                        }
                    }
                }
            } else Row(Modifier.weight(1f)) {
                Box(if (twoPane) Modifier.weight(0.42f) else Modifier.weight(1f)) {
                    // One pane: the list stays under the open message, so it keeps its place and predictive back shows it.
                    Box(if (!twoPane && model.selectedId != null) Modifier.clearAndSetSemantics {} else Modifier) { Inbox(model, onRetry) }
                    if (!twoPane) ReadingPane(model, single = true)
                }
                if (twoPane) {
                    VerticalDivider()
                    Box(Modifier.weight(0.58f)) { ReadingPane(model, single = false) }
                }
            }
        }
    }
}

/** The Sent, Done, Later and Deleted notice with Undo: a snackbar that springs up and fades away. */
@Composable private fun Notice(model: MailModel) {
    val notice = model.notice?.let { it to (model.undoSendId != null || model.undoChanges.isNotEmpty()) }
    AnimatedContent(notice, label = "notice", transitionSpec = {
        (fadeIn(Fade) + slideInVertically(landing(IntOffset.VisibilityThreshold)) { it / 2 }) togetherWith fadeOut(Fade) using SizeTransform(clip = false)
    }) { shown ->
        if (shown == null) return@AnimatedContent
        val (text, undo) = shown
        // The theme sets no inverse colours, so the snackbar inverts the surface itself.
        val ink = MaterialTheme.colorScheme.surface
        Snackbar(Modifier.padding(12.dp).semantics { liveRegion = LiveRegionMode.Polite },
            containerColor = MaterialTheme.colorScheme.onSurface, contentColor = ink, action = {
                val colors = ButtonDefaults.textButtonColors(contentColor = ink)
                val sent = model.undoSendId != null
                if (undo) TextButton(enabled = if (sent) !model.sending else !model.working, colors = colors,
                    onClick = { model.perform { if (sent) model.undoSend() else model.undoAction() } },
                    modifier = Modifier.sizeIn(minWidth = 48.dp, minHeight = 48.dp).semantics { contentDescription = if (sent) "Undo send" else "Undo" }) {
                    Text("Undo", fontWeight = FontWeight.SemiBold)
                } else TextButton(colors = colors, onClick = { model.notice = null }) { Text("Dismiss", fontWeight = FontWeight.SemiBold) }
            }) { Text(text) }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable private fun Inbox(model: MailModel, onRetry: (SendFailure) -> Unit) {
    Column(Modifier.fillMaxSize()) {
        OutlinedTextField(model.searchText, { model.perform { model.changeSearch(it) } },
            label = { Text("Search") }, singleLine = true,
            keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, imeAction = ImeAction.Search),
            trailingIcon = {
                if (model.searchText.isNotEmpty()) TextButton(onClick = { model.perform { model.changeSearch("") } },
                    modifier = Modifier.sizeIn(minWidth = 48.dp, minHeight = 48.dp).semantics { contentDescription = "Clear search" }) { Text("Clear") }
            }, modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp).semantics { contentDescription = "Search" })
        Row(Modifier.padding(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            if (model.mailbox == Mailbox.TRASH && model.folder == null) {
                // In Trash the filter makes way for Empty Trash, as on desktop.
                var confirm by remember { mutableStateOf(false) }
                TextButton(enabled = model.rows.isNotEmpty() && !model.working, onClick = { confirm = true },
                    modifier = Modifier.heightIn(min = 48.dp)) { Text(if (model.emptyingTrash) "Emptying…" else "Empty Trash") }
                if (confirm) AlertDialog(onDismissRequest = { confirm = false }, title = { Text("Delete all permanently?") },
                    confirmButton = { TextButton(onClick = { confirm = false; model.perform { model.emptyTrash() } },
                        colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error)) { Text("Delete all") } },
                    dismissButton = { TextButton(onClick = { confirm = false }) { Text("Cancel") } })
            } else {
                FilterChip(!model.unreadOnly, { model.unreadOnly = false }, label = { Text("All") },
                    modifier = Modifier.sizeIn(minWidth = 48.dp, minHeight = 48.dp).semantics { contentDescription = "All" })
                FilterChip(model.unreadOnly, { model.unreadOnly = true }, label = { Text("Unread") },
                    modifier = Modifier.sizeIn(minWidth = 48.dp, minHeight = 48.dp).semantics { contentDescription = "Unread" })
            }
        }
        // A pull shows the indicator at once; a sync the app starts shows it only past 150 ms.
        var pulled by remember { mutableStateOf(false) }
        PullToRefreshBox(isRefreshing = pulled || afterWait(model.syncing), onRefresh = {
            pulled = true
            model.perform { try { model.sync() } finally { pulled = false } }
        }, modifier = Modifier.weight(1f)) {
            val rows = model.visibleRows
            val ids = rows.map { it.id }
            val shownIds = remember { ArrayList<Long>() }
            val animate = remember(ids) { smallChange(shownIds, ids).also { shownIds.clear(); shownIds.addAll(ids) } }
            val list = rememberLazyListState()
            val atTop by remember(list) { derivedStateOf { list.firstVisibleItemIndex == 0 && list.firstVisibleItemScrollOffset == 0 } }
            LazyColumn(Modifier.fillMaxSize(), state = list) {
                // motion.md: at the top, new mail grows in where it can be seen; scrolled down, the first row on screen stays put.
                if (atTop) list.requestScrollToItem(0)
                if (rows.isEmpty() && model.failures.isEmpty() && !model.loading) item(key = "empty") {
                    EmptyInbox(model, rowMotion(true).fillParentMaxSize())
                }
                items(model.failures, key = { "failure-${it.id}" }) { failure ->
                    ListItem(headlineContent = { Text("Outbox: ${failure.subject}") }, supportingContent = { Text(failure.message) },
                        modifier = rowMotion(animate).clickable { onRetry(failure) })
                }
                items(rows, key = { it.id }) { row ->
                    Column(rowMotion(animate)) {
                        MailListRow(row, model.working, model.mailbox != Mailbox.TRASH, model.moveTargets(row.account),
                            onSelect = { model.perform { model.select(row.id) } },
                            onAction = { model.perform { model.act(it, row.id) } },
                            onRead = { model.perform { model.setRead(row) } },
                            onMove = { folder -> model.perform { model.move(row.id, folder) } })
                        HorizontalDivider()
                    }
                }
                if (model.hasMore) item(key = "more") {
                    TextButton(onClick = { model.perform { model.reload(more = true) } },
                        modifier = rowMotion(animate).fillMaxWidth().heightIn(min = 48.dp).semantics { contentDescription = "Load more" }) { Text("Load more") }
                }
            }
        }
    }
}

/** motion.md "Inbox reaches zero": a calm state that fades in. No confetti. */
@Composable private fun EmptyInbox(model: MailModel, modifier: Modifier) {
    val title = when {
        model.unreadOnly && model.rows.isNotEmpty() -> "No unread mail"
        model.searchText.isNotBlank() -> "No results"
        model.mailbox == Mailbox.INBOX && model.folder == null -> "Nothing left."
        else -> "No mail here"
    }
    Column(modifier.padding(24.dp), verticalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterVertically),
        horizontalAlignment = Alignment.CenterHorizontally) {
        Text(title, style = MaterialTheme.typography.headlineSmall, textAlign = TextAlign.Center)
        Text(if (afterWait(model.syncing)) "Checking for mail…" else "Pull to refresh your mail.",
            color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = TextAlign.Center)
    }
}

private fun LazyItemScope.rowMotion(animate: Boolean): Modifier =
    if (animate) Modifier.animateItem(fadeInSpec = Fade, placementSpec = landing(IntOffset.VisibilityThreshold), fadeOutSpec = Fade) else Modifier

// Material's check mark; the app has no icon library.
private val CheckIcon = ImageVector.Builder("Done", 24.dp, 24.dp, 24f, 24f)
    .addPath(addPathNodes("M9 16.17L4.83 12l-1.42 1.41L9 19 21 7l-1.41-1.41z"), fill = SolidColor(Color.Black)).build()

@OptIn(ExperimentalMaterial3Api::class, ExperimentalFoundationApi::class)
@Composable private fun MailListRow(row: MailRow, busy: Boolean, canDelete: Boolean, targets: List<MailFolder>, onSelect: () -> Unit,
    onAction: (MailAction) -> Unit, onRead: () -> Unit, onMove: (MailFolder) -> Unit) {
    // A long press opens what iOS's context menu has: Mark read or unread, and Move to.
    var menu by remember { mutableStateOf(false) }
    var moving by remember { mutableStateOf(false) }
    val readLabel = if (row.unread) "Mark read" else "Mark unread"
    val currentAction by rememberUpdatedState(onAction)
    val currentBusy by rememberUpdatedState(busy)
    // motion.md: a release past 120 dp acts; short of it the row springs back.
    val reach = with(LocalDensity.current) { 120.dp.toPx() }
    val state = rememberSwipeToDismissBoxState(confirmValueChange = { value ->
        if (!currentBusy && value == SwipeToDismissBoxValue.StartToEnd) currentAction(MailAction.Done)
        value == SwipeToDismissBoxValue.Settled || !currentBusy
    }, positionalThreshold = { reach })
    val scope = rememberCoroutineScope()
    val haptics = LocalHapticFeedback.current
    // Colour and haptics use the release line, so what the row shows is what letting go does.
    val past by remember(state, reach) { derivedStateOf { abs(runCatching { state.requireOffset() }.getOrDefault(0f)) >= reach } }
    LaunchedEffect(past) { if (past) haptics.performHapticFeedback(HapticFeedbackType.GestureThresholdActivate) }
    // The row stays out while Done runs and leaves with the list; if Done failed, it comes back. Check the offset, not
    // currentValue: once Done starts, busy vetoes the settle, so the row can rest at the edge while still "Settled".
    LaunchedEffect(busy) { if (!busy && state.dismissDirection == SwipeToDismissBoxValue.StartToEnd) state.reset() }
    val revealed = state.currentValue == SwipeToDismissBoxValue.EndToStart
    val view = RowPresentation(row)
    SwipeToDismissBox(state = state, gesturesEnabled = !busy, backgroundContent = {
        val colors = MaterialTheme.colorScheme
        val fill by animateColorAsState(if (past) colors.tertiary else colors.surfaceVariant, tween(100, easing = EaseOut), label = "swipe fill")
        val ink by animateColorAsState(if (past) colors.onTertiary else colors.onSurfaceVariant, tween(100, easing = EaseOut), label = "swipe ink")
        if (state.dismissDirection == SwipeToDismissBoxValue.StartToEnd) Row(Modifier.fillMaxSize().background(fill).padding(horizontal = 20.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
            Icon(CheckIcon, contentDescription = null, tint = ink)
            Text("Done", color = ink)
        } else Row(Modifier.fillMaxSize().background(colors.surfaceVariant)
            .then(if (revealed) Modifier else Modifier.clearAndSetSemantics {}),
            horizontalArrangement = Arrangement.End, verticalAlignment = Alignment.CenterVertically) {
            TextButton(enabled = !busy, onClick = { scope.launch { state.reset(); currentAction(MailAction.Later) } },
                modifier = Modifier.sizeIn(minWidth = 48.dp, minHeight = 48.dp).semantics { contentDescription = "Later" }) { Text("Later") }
            if (canDelete) TextButton(enabled = !busy, onClick = { scope.launch { state.reset(); currentAction(MailAction.Delete) } },
                colors = ButtonDefaults.textButtonColors(contentColor = colors.error),
                modifier = Modifier.sizeIn(minWidth = 48.dp, minHeight = 48.dp).semantics { contentDescription = "Delete" }) { Text("Delete") }
            TextButton(onClick = { scope.launch { state.reset() } },
                modifier = Modifier.sizeIn(minWidth = 48.dp, minHeight = 48.dp).semantics { contentDescription = "Cancel" }) { Text("Cancel") }
        }
    }) {
        Box {
        ListItem(modifier = Modifier.combinedClickable(onLongClickLabel = "More actions", onLongClick = { if (!busy) menu = true }, onClick = onSelect)
            .then(if (revealed) Modifier.clearAndSetSemantics {} else Modifier.semantics {
            stateDescription = if (row.unread) "Unread" else "Read"
            customActions = listOf(CustomAccessibilityAction("Done") { if (!busy) onAction(MailAction.Done); !busy },
                CustomAccessibilityAction("Later") { if (!busy) onAction(MailAction.Later); !busy }) +
                (if (canDelete) listOf(CustomAccessibilityAction("Delete") { if (!busy) onAction(MailAction.Delete); !busy }) else emptyList()) +
                CustomAccessibilityAction(readLabel) { if (!busy) onRead(); !busy } +
                targets.map { folder -> CustomAccessibilityAction("Move to ${folder.path}") { if (!busy) onMove(folder); !busy } }
        }), headlineContent = { Text(view.sender, fontWeight = if (row.unread) FontWeight.Bold else FontWeight.Normal, maxLines = 1, overflow = TextOverflow.Ellipsis) },
            supportingContent = { Column {
                Text(view.subject, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(row.preview, maxLines = 2, overflow = TextOverflow.Ellipsis)
            } },
            trailingContent = { Column(horizontalAlignment = Alignment.End) {
                Text(DateTimeFormatter.ofLocalizedDate(FormatStyle.SHORT).withZone(ZoneId.systemDefault()).format(view.date), style = MaterialTheme.typography.labelSmall)
                if (row.messageCount > 1u) Badge {
                    Text(row.messageCount.toString(), Modifier.semantics { contentDescription = "${row.messageCount} messages" })
                }
                if (row.hasAttachments) Text("Attachment", style = MaterialTheme.typography.labelSmall)
            } })
        DropdownMenu(menu || moving, { menu = false; moving = false }) {
            if (moving) targets.forEach { folder ->
                DropdownMenuItem(text = { Text(folder.path) }, onClick = { moving = false; onMove(folder) })
            } else {
                DropdownMenuItem(text = { Text(readLabel) }, onClick = { menu = false; onRead() })
                if (targets.isNotEmpty()) DropdownMenuItem(text = { Text("Move to…") }, onClick = { menu = false; moving = true })
            }
        }
        }
    }
}

private data class Reading(val id: Long, val messages: List<MailMessage>)

/** The open conversation. One pane: it slides in from the side and follows predictive back. Two panes: it cross-fades. */
@Composable private fun ReadingPane(model: MailModel, single: Boolean) {
    val reading = model.selectedId?.let { Reading(it, model.messages) }
    AnimatedContent(reading, Modifier.fillMaxSize(), contentKey = { it?.id }, label = "reading", transitionSpec = {
        when {
            !single -> fadeIn(Fade) togetherWith fadeOut(Fade)
            targetState != null -> (fadeIn(Fade) + slideInHorizontally(landing(IntOffset.VisibilityThreshold)) { it / 10 }) togetherWith fadeOut(Fade)
            else -> EnterTransition.None togetherWith (fadeOut(Fade) + slideOutHorizontally(tween(150, easing = EaseOut)) { it / 10 })
        } using null
    }) { shown ->
        if (shown == null) {
            if (!single) Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { Text("Select a message") }
            return@AnimatedContent
        }
        val back = rememberBackPreview(enabled = shown.id == model.selectedId && model.draft == null && model.accountForm == null) {
            model.clearSelection(); false
        }
        // Surface also keeps touches from reaching the list underneath.
        Surface(Modifier.fillMaxSize().backPreview(back)) {
            Column(Modifier.fillMaxSize()) {
                if (single) TextButton(onClick = model::clearSelection) { Text("Back") }
                Box(Modifier.weight(1f)) {
                    if (shown.messages.isEmpty()) MessagePlaceholder(model.rows.firstOrNull { it.id == shown.id })
                    else ConversationScreen(model, shown.messages)
                }
            }
        }
    }
}

/** One quiet line above the body. Nothing runs before the confirm. */
// shortcut: no "Move all from this sender to Trash" offer; there is no sender-wide bulk move to reuse yet.
@Composable private fun UnsubscribeLine(model: MailModel, id: Long, offer: UnsubscribeOffer) {
    var confirm by remember(id) { mutableStateOf(false) }
    val context = LocalContext.current
    val quiet = MaterialTheme.typography.bodySmall
    when {
        id in model.unsubscribedIds -> Text("Unsubscribed", style = quiet)
        model.unsubscribingId == id -> Text("Unsubscribing…", style = quiet)
        else -> TextButton(onClick = { confirm = true }, modifier = Modifier.heightIn(min = 48.dp)) {
            Text("Unsubscribe from ${offer.list}", style = quiet, maxLines = 2)
        }
    }
    if (confirm) AlertDialog(
        onDismissRequest = { confirm = false },
        title = { Text("Unsubscribe from ${offer.list}?") },
        text = { Text(offer.explain) },
        confirmButton = { TextButton(onClick = {
            confirm = false
            val link = offer.link
            if (link == null) model.perform { model.unsubscribe(id) }
            else if (link.startsWith("https://")) {
                try { context.startActivity(Intent(Intent.ACTION_VIEW, android.net.Uri.parse(link))) }
                catch (_: ActivityNotFoundException) { model.error = "No browser can open this page" }
            }
        }) { Text("Unsubscribe") } },
        dismissButton = { TextButton(onClick = { confirm = false }) { Text("Cancel") } },
    )
}

/** motion.md "Loading": nothing for the first 150 ms, then the row's preview stands in for the body. */
@Composable private fun MessagePlaceholder(row: MailRow?) {
    if (!afterWait(true)) return
    val quiet = MaterialTheme.colorScheme.onSurfaceVariant
    Column(Modifier.fillMaxSize().padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        if (row == null) { Text("Loading message…", color = quiet); return@Column }
        val view = RowPresentation(row)
        Text(view.subject, style = MaterialTheme.typography.titleLarge)
        Text(view.sender)
        Text(row.preview, color = quiet)
    }
}

@Composable private fun ConversationScreen(model: MailModel, messages: List<MailMessage>) {
    // Most mail is a single message: keep the full-height reading view for it.
    messages.singleOrNull()?.let { MessageScreen(model, it); return }
    LazyColumn(Modifier.fillMaxSize()) {
        items(messages, key = { it.row.id }) { message ->
            val row = message.row
            val view = RowPresentation(row)
            val expanded = row.id in model.expandedIds
            Row(Modifier.fillMaxWidth().heightIn(min = 48.dp)
                .clickable(role = Role.Button, onClickLabel = "${if (expanded) "Collapse" else "Expand"} message from ${view.sender}") {
                    model.perform { model.toggleMessage(row.id) }
                }.padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(if (expanded) "⌄" else "›")
                Text(view.sender, Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis, fontWeight = if (row.unread) FontWeight.Bold else FontWeight.Normal)
                Text(row.preview, Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall)
                Text(DateTimeFormatter.ofLocalizedDate(FormatStyle.SHORT).withZone(ZoneId.systemDefault()).format(view.date), style = MaterialTheme.typography.labelSmall)
            }
            // shortcut: like desktop, expanded bodies scroll independently; measure HTML for continuous scrolling later.
            if (expanded) Box(Modifier.height(560.dp)) { MessageScreen(model, message) }
            HorizontalDivider()
        }
    }
}

@Composable private fun MessageScreen(model: MailModel, message: MailMessage) {
    var plain by remember(message.row.id) { mutableStateOf(false) }
    val context = LocalContext.current
    fun openAttachment(part: MailAttachment, share: Boolean = false) {
        model.perform {
            val file = model.openAttachment(message.row.id, part) ?: return@perform
            val uri = FileProvider.getUriForFile(context, "${context.packageName}.files", file)
            // From the extension the user sees, as FileProvider reports it, not the sender's claim:
            // invoice.pdf sent as text/html would open in a browser.
            val mime = MimeTypeMap.getSingleton().getMimeTypeFromExtension(file.extension.lowercase(java.util.Locale.ROOT))
                ?: "application/octet-stream"
            val intent = if (share) Intent(Intent.ACTION_SEND).setType(mime).putExtra(Intent.EXTRA_STREAM, uri)
                else Intent(Intent.ACTION_VIEW).setDataAndType(uri, mime)
            intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            intent.clipData = ClipData.newRawUri(file.name, uri)
            try {
                if (!share && intent.resolveActivity(context.packageManager) == null) throw ActivityNotFoundException()
                context.startActivity(Intent.createChooser(intent, if (share) "Share attachment" else "Open attachment"))
            } catch (_: ActivityNotFoundException) { model.error = "No app can open this file" }
        }
    }
    Column(Modifier.fillMaxSize().padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        SelectionContainer {
            Column {
                Text(message.row.subject.ifEmpty { "(No subject)" }, style = MaterialTheme.typography.titleLarge)
                Text(message.row.senderName)
                Text(message.row.senderAddress, style = MaterialTheme.typography.bodySmall)
                Text(DateTimeFormatter.ofLocalizedDateTime(FormatStyle.MEDIUM).withZone(ZoneId.systemDefault()).format(RowPresentation(message.row).date), style = MaterialTheme.typography.labelSmall)
            }
        }
        message.unsubscribe?.let { UnsubscribeLine(model, message.row.id, it) }
        if (message.htmlBody != null) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Checkbox(plain, { plain = it }); Text("Plain text")
                if (message.row.id !in model.remoteImagesIds) TextButton(onClick = { model.loadRemoteImages(message.row.id) }) { Text("Load remote images") }
            }
        }
        Box(Modifier.weight(1f)) {
            if (!plain && message.htmlBody != null) MailHtml(message.row.id, message.htmlBody!!, message.row.id in model.remoteImagesIds)
            else SelectionContainer { Text(message.textBody, Modifier.fillMaxSize().verticalScroll(rememberScrollState())) }
        }
        if (message.attachments.isNotEmpty()) {
            LazyColumn(Modifier.heightIn(max = 144.dp)) {
                items(message.attachments, key = { it.section }) { part ->
                    val downloading = (message.row.id to part.section) in model.downloadingAttachments
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Row(Modifier.weight(1f).heightIn(min = 48.dp)
                            .clickable(enabled = !downloading, onClickLabel = "Open", role = Role.Button) { openAttachment(part) }
                            .padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            if (afterWait(downloading)) CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                            Text("${safeAttachmentName(part.filename)} · ${part.size} bytes", style = MaterialTheme.typography.bodySmall)
                        }
                        TextButton(enabled = !downloading, onClick = { openAttachment(part, share = true) },
                            modifier = Modifier.sizeIn(minWidth = 48.dp, minHeight = 48.dp)
                                .semantics { contentDescription = "Share ${safeAttachmentName(part.filename)}" }) { Text("Share") }
                    }
                }
            }
        }
        FlowRow(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
            TextButton(enabled = !model.working, onClick = { model.perform { model.reply(message.row.id) } }) { Text("Reply") }
            TextButton(enabled = !model.working, onClick = { model.perform { model.reply(message.row.id, ComposeKind.REPLY_ALL) } },
                modifier = Modifier.sizeIn(minWidth = 48.dp, minHeight = 48.dp).semantics { contentDescription = "Reply all" }) { Text("Reply all") }
            TextButton(enabled = !model.working, onClick = { model.perform { model.reply(message.row.id, ComposeKind.FORWARD) } },
                modifier = Modifier.sizeIn(minWidth = 48.dp, minHeight = 48.dp).semantics { contentDescription = "Forward" }) { Text("Forward") }
            val readLabel = if (message.row.unread) "Mark read" else "Mark unread"
            TextButton(enabled = !model.working, onClick = { model.perform { model.setRead(message.row.id, message.row.unread) } },
                modifier = Modifier.sizeIn(minWidth = 48.dp, minHeight = 48.dp).semantics { contentDescription = readLabel }) { Text(readLabel) }
            Box {
                var choices by remember { mutableStateOf<List<LaterChoice>>(emptyList()) }
                TextButton(enabled = !model.working, onClick = { choices = model.core?.laterChoices().orEmpty() },
                    modifier = Modifier.sizeIn(minWidth = 48.dp, minHeight = 48.dp)) { Text("Later") }
                DropdownMenu(expanded = choices.isNotEmpty(), onDismissRequest = { choices = emptyList() }) {
                    choices.forEach { choice ->
                        DropdownMenuItem(text = { Text(choice.label) }, onClick = {
                            choices = emptyList()
                            model.perform { model.act(MailAction.Later, message.row.id, conversation = false, until = choice.untilUnix) }
                        })
                    }
                }
            }
            val targets = model.moveTargets(message.row.account)
            if (targets.isNotEmpty()) Box {
                var open by remember { mutableStateOf(false) }
                TextButton(enabled = !model.working, onClick = { open = true },
                    modifier = Modifier.sizeIn(minWidth = 48.dp, minHeight = 48.dp)) { Text("Move to") }
                DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
                    targets.forEach { folder ->
                        DropdownMenuItem(text = { Text(folder.path) }, onClick = {
                            open = false
                            model.perform { model.move(message.row.id, folder, conversation = false) }
                        })
                    }
                }
            }
            TextButton(enabled = !model.working, onClick = { model.perform { model.act(MailAction.Delete, message.row.id, conversation = false) } }) { Text("Delete") }
            Button(enabled = !model.working, onClick = { model.perform { model.act(MailAction.Done, message.row.id, conversation = false) } }) { Text("Done") }
        }
    }
}
