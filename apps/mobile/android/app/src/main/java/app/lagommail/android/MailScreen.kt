package app.lagommail.android

import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.Intent
import androidx.annotation.DrawableRes
import android.webkit.MimeTypeMap
import androidx.core.content.FileProvider
import androidx.compose.animation.*
import androidx.compose.animation.core.*
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.draw.clip
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
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
import androidx.compose.runtime.saveable.rememberSaveable
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
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.semantics.*
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import app.lagommail.core.*
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.flow.drop
import kotlinx.coroutines.launch
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle
import kotlin.math.roundToInt

@Composable internal fun MailScreen(model: MailModel, twoPane: Boolean) {
    var retrying by remember { mutableStateOf<SendFailure?>(null) }
    var settings by rememberSaveable { mutableStateOf(false) }
    // A mailto link, a share or a notification (a new draft or an opened message) leaves Settings; Add account comes back to it.
    // Only changes count, as in LagomHome: after a rotation a selection that was already there keeps Settings open.
    LaunchedEffect(Unit) {
        snapshotFlow { (model.draft != null) to model.selectedId }.drop(1)
            .collect { (draft, selected) -> if (draft || selected != null) settings = false }
    }
    // Compose and Add account cover the mail screen instead of replacing it, so the list keeps its
    // place. The form or draft rides along so a leaving screen keeps its content while it animates out.
    val overlay: Any? = model.accountForm ?: model.draft
    val rise = with(LocalDensity.current) { 12.dp.roundToPx() }
    Box(Modifier.fillMaxSize()) {
        Box(if (overlay != null || settings) Modifier.clearAndSetSemantics {} else Modifier) {
            MailHome(model, twoPane, onRetry = { retrying = it }, onSettings = { settings = true })
        }
        // Under Compose and Add account, so adding an account from Settings comes back to it.
        if (settings) Box(if (overlay != null) Modifier.clearAndSetSemantics {} else Modifier) { SettingsScreen(model) { settings = false } }
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
    model.newFolder?.let { request -> NewFolderDialog(model, request) }
    retrying?.let { failure ->
        AlertDialog(onDismissRequest = { retrying = null }, title = { Text("Send again?") },
            text = { Text("Check Sent first if delivery was uncertain. This can send a duplicate.") },
            confirmButton = { TextButton(onClick = { retrying = null; model.perform { model.core?.retrySend(failure.id); model.sync() } }) { Text("Send again") } },
            dismissButton = { TextButton(onClick = { retrying = null }) { Text("Cancel") } })
    }
}

/** The mailboxes in the order of every lagom app, desktop's first; null is Drafts, which core keeps apart. */
internal val MailboxOrder: List<Mailbox?> = listOf(Mailbox.INBOX, Mailbox.LATER, Mailbox.SENT, Mailbox.DONE, null, Mailbox.TRASH)

internal fun Mailbox.label(): String = when (this) {
    Mailbox.INBOX -> "Inbox"; Mailbox.LATER -> "Later"; Mailbox.DONE -> "Done"; Mailbox.SENT -> "Sent"; Mailbox.TRASH -> "Trash"
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable private fun MailHome(model: MailModel, twoPane: Boolean, onRetry: (SendFailure) -> Unit, onSettings: () -> Unit) {
    var menu by remember { mutableStateOf(false) }
    Box(Modifier.fillMaxSize()) {
    Box(if (!twoPane && model.selectedId != null) Modifier.clearAndSetSemantics {} else Modifier) {
    Scaffold(containerColor = lagom.bg, topBar = {
        // As on iOS (Alex 2026-11-20): the lagom mark, and on the right the title and a quiet line that open the
        // mailboxes and accounts. The filter, Search and New message follow in the list's own row.
        TopBar(Modifier.statusBarsPadding(), end = 8.dp) {
            AppIcon(28.dp)
            Spacer(Modifier.weight(1f))
            Box {
                val title = if (model.showingDrafts) "Drafts" else model.folder?.name ?: model.mailbox.label()
                val syncing = afterWait(model.syncing)
                val left = if (!model.showingDrafts && model.mailbox == Mailbox.INBOX && model.folder == null && model.accounts.isNotEmpty())
                    model.counts.filter { model.accountId == null || it.account == model.accountId }.sumOf { it.inbox } else null
                val status = syncStatus(if (syncing && model.syncing) SyncState.Syncing else model.syncState, left)
                Column(Modifier.clip(RoundedCornerShape(Radius.control)).clickable(onClickLabel = "Mailboxes and accounts") { menu = true }
                    .padding(horizontal = 8.dp, vertical = 4.dp).semantics(mergeDescendants = true) {}, horizontalAlignment = Alignment.End) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(title, Modifier.semantics { heading() }, style = MaterialTheme.typography.titleLarge.copy(fontFamily = DisplayFace, fontWeight = FontWeight.Bold), maxLines = 1, overflow = TextOverflow.Ellipsis)
                        LagomIcon(R.drawable.ic_chevron, Modifier.padding(start = 4.dp), size = 14.dp)
                    }
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(5.dp)) {
                        // Synced is the quiet default: the cloud shows only while it syncs or fails.
                        if (status.icon != "cloud-synced") {
                            LagomIcon(if (status.icon == "cloud-failed") R.drawable.ic_cloud_failed else R.drawable.ic_cloud_syncing, size = 14.dp)
                        }
                        Text(status.text, color = lagom.ink2, style = MaterialTheme.typography.bodySmall, maxLines = 1,
                            overflow = TextOverflow.Ellipsis)
                    }
                }
                DropdownMenu(menu, { menu = false }) {
                    MailboxOrder.forEach { box ->
                        DropdownMenuItem(text = { Text(box?.label() ?: "Drafts") }, onClick = {
                            menu = false; model.perform { if (box == null) model.showDrafts() else model.changeScope(model.accountId, box) }
                        })
                    }
                    // Desktop's folder group: the server's own folders, with the account when there are several.
                    val folders = model.folders.filter { model.accountId == null || it.account == model.accountId }
                    val owners = model.accounts.filter { model.accountId == null || it.id == model.accountId }
                    if (owners.isNotEmpty()) HorizontalDivider()
                    folders.forEach { folder ->
                        val account = model.accounts.find { it.id == folder.account }?.takeIf { model.accounts.size > 1 }
                        DropdownMenuItem(text = { Text(account?.let { "${folder.path} · ${it.displayName}" } ?: folder.path) }, onClick = {
                            menu = false; model.perform { model.openFolder(folder) }
                        })
                    }
                    owners.forEach { account ->
                        DropdownMenuItem(text = { Text(if (owners.size > 1) "New folder in ${account.displayName}" else "New folder") },
                            enabled = !model.working, onClick = { menu = false; model.newFolder = NewFolder(account.id) })
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
                    // The menu is a phone's sidebar, so Settings closes it as on the Mac; adding, naming, signing and
                    // removing accounts live there (Alex 2026-11-20).
                    HorizontalDivider()
                    DropdownMenuItem(text = { Text("Settings") }, leadingIcon = { LagomIcon(R.drawable.ic_settings) },
                        onClick = { menu = false; onSettings() })
                }
            }
        }
    }) { padding ->
        Column(Modifier.fillMaxSize().padding(padding)) {
            if (model.showingDrafts) {
                ListBar(model)
                LazyColumn(Modifier.weight(1f)) {
                    val drafts = model.drafts.filter { model.accountId == null || it.account == model.accountId }
                    if (drafts.isEmpty()) item(key = "empty") { EmptyState(R.drawable.ic_edit_draft, "No drafts", rowMotion(true).fillParentMaxSize()) }
                    items(drafts, key = { it.id }) { draft ->
                        Column(rowMotion(true).fillMaxWidth().heightIn(min = 48.dp).clickable(onClickLabel = "Edit draft") { model.perform { model.openDraft(draft) } }
                            .padding(horizontal = 16.dp, vertical = 10.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                            Text(draft.subject.ifEmpty { "(No subject)" }, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            Text(draft.to, color = lagom.ink2, style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            Text(listOfNotNull(model.accounts.find { it.id == draft.account }?.email, draft.updatedUnix.takeIf { it > 0 }?.let {
                                DateTimeFormatter.ofLocalizedDateTime(FormatStyle.SHORT).withZone(ZoneId.systemDefault()).format(java.time.Instant.ofEpochSecond(it))
                            }).joinToString(" · "), color = lagom.ink2, style = MaterialTheme.typography.bodySmall)
                        }
                    }
                }
            } else Row(Modifier.weight(1f)) {
                Box(if (twoPane) Modifier.weight(0.42f) else Modifier.weight(1f)) { Inbox(model, onRetry) }
                if (twoPane) {
                    VerticalDivider()
                    Box(Modifier.weight(0.58f)) { ReadingPane(model, single = false) }
                }
            }
        }
    }
    }
    // One pane: the message covers the list and its bar, which stay underneath, so the list keeps its place and
    // predictive back shows it.
    if (!twoPane && !model.showingDrafts) ReadingPane(model, single = true)
    // Above the open message too: Undo after a reply or Done must stay reachable.
    Box(Modifier.align(Alignment.BottomCenter)) { Notice(model) }
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
        ListBar(model)
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
            val dates = mailDates()
            val atTop by remember(list) { derivedStateOf { list.firstVisibleItemIndex == 0 && list.firstVisibleItemScrollOffset == 0 } }
            LazyColumn(Modifier.fillMaxSize(), state = list) {
                // motion.md: at the top, new mail grows in where it can be seen; scrolled down, the first row on screen stays put.
                if (atTop) list.requestScrollToItem(0)
                // Until the first read or the first sync lands, quiet placeholder rows stand in, not inbox zero (desktop #141).
                if (rows.isEmpty() && model.failures.isEmpty()) item(key = "empty") {
                    if (model.rows.isEmpty() && (model.loading || !model.synced)) LoadingRows(rowMotion(true))
                    else EmptyInbox(model, rowMotion(true).fillParentMaxSize())
                }
                if (model.failures.isNotEmpty()) item(key = "failures") { GroupHeader("Outbox needs attention", rowMotion(animate)) }
                items(model.failures, key = { "failure-${it.id}" }) { failure ->
                    Column(rowMotion(animate).fillMaxWidth().clickable(onClickLabel = "Send again") { onRetry(failure) }
                        .padding(horizontal = 16.dp, vertical = 10.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                        Text(failure.subject.ifEmpty { "(No subject)" }, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text(failure.message, color = lagom.danger, style = MaterialTheme.typography.bodySmall)
                    }
                }
                // Search lists in match order, so a group can come back; a repeat keys on its first row.
                val titles = HashSet<String>()
                grouped(rows) { dates.group(RowPresentation(it).date) }.forEach { (title, groupRows) ->
                    item(key = if (titles.add(title)) "group-$title" else "group-$title-${groupRows.first().id}") {
                        GroupHeader(title, rowMotion(animate))
                    }
                    items(groupRows, key = { it.id }) { row ->
                        Box(rowMotion(animate)) {
                            MailListRow(row, model.working, model.mailbox != Mailbox.TRASH, model.moveTargets(row.account), accountMark(model, row.account),
                                onSelect = { model.perform { model.select(row.id) } },
                                onAction = { model.perform { model.act(it, row.id) } },
                                onRead = { model.perform { model.setRead(row) } },
                                onMove = { folder -> model.perform { model.move(row.id, folder) } },
                                onNewFolder = { model.newFolder = NewFolder(row.account, row.id) })
                        }
                    }
                }
                if (model.hasMore) item(key = "more") {
                    Box(rowMotion(animate).fillMaxWidth().padding(vertical = 8.dp), contentAlignment = Alignment.Center) {
                        HouseButton("Load more", R.drawable.ic_load_more) { model.perform { model.reload(more = true) } }
                    }
                }
            }
        }
    }
}

/** An account's mark, only when several accounts share the list: the three accents in turn, as desktop `account_accent`. */
@Composable private fun accountMark(model: MailModel, account: String): Color? {
    if (model.accounts.size < 2) return null
    val index = model.accounts.indexOfFirst { it.id == account }.coerceAtLeast(0)
    return accountAccent(index)
}

/** A list's group, "Outbox needs attention". */
@Composable private fun GroupHeader(title: String, modifier: Modifier = Modifier) =
    Text(title, modifier.fillMaxWidth().padding(start = 16.dp, top = 14.dp, bottom = 4.dp).semantics { heading() },
        color = lagom.ink2, style = MaterialTheme.typography.bodySmall)

/** motion.md "Inbox reaches zero": the inbox icon, one line and one quiet line, fading in. No confetti. */
@Composable private fun EmptyInbox(model: MailModel, modifier: Modifier) {
    when {
        model.unreadOnly && model.searchText.isBlank() && model.rows.isNotEmpty() -> EmptyState(R.drawable.ic_read, "No unread mail", modifier)
        model.searchText.isNotBlank() -> EmptyState(R.drawable.ic_search, "No results", modifier, "Nothing in your mail matches.")
        model.folder != null -> EmptyState(R.drawable.ic_inbox, "No mail here", modifier, "Pull to refresh your mail.")
        // Later and Done fill from your own actions, not a refresh; each mailbox says what lands there. Same words on iOS.
        else -> when (model.mailbox) {
            Mailbox.INBOX -> EmptyState(R.drawable.ic_inbox, "Nothing left.", modifier, "Not too much. Not too little. Just right.")
            Mailbox.LATER -> EmptyState(R.drawable.ic_later, "Nothing waiting.", modifier, "Mail you put off waits here.")
            Mailbox.SENT -> EmptyState(R.drawable.ic_sent, "Nothing sent.", modifier, "Mail you send lands here.")
            Mailbox.DONE -> EmptyState(R.drawable.ic_done, "Nothing done yet.", modifier, "Mail you mark Done lands here.")
            Mailbox.TRASH -> EmptyState(R.drawable.ic_trash, "Trash is empty.", modifier, "Deleted mail lands here.")
        }
    }
}

/** The list's row under the bar, as iOS: the filter (Empty Trash in Trash) on the left, Search and New message on the right.
 *  Search opens into the field in the row's place and stays open while it holds text. Drafts have New message only. */
@Composable private fun ListBar(model: MailModel) {
    var searching by remember { mutableStateOf(false) }
    val drafts = model.showingDrafts
    // The glyphs on the right end 16 dp in, under the mailbox title: 2 dp here and 14 dp inside a 48 dp button.
    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).padding(start = 16.dp, end = 2.dp, top = 4.dp, bottom = 4.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        if (!drafts && (searching || model.searchText.isNotEmpty())) {
            val focus = remember { FocusRequester() }
            // False only once the field has had focus and lost it; an empty field then closes, as on iOS.
            var focused by remember { mutableStateOf<Boolean?>(null) }
            SearchField(model.searchText, focus, Modifier.weight(1f).onFocusChanged {
                focused = if (it.hasFocus) true else if (focused == true) false else focused
            }) { model.perform { model.changeSearch(it) } }
            // The keyboard rises with the field the button opened, not with a search that is only kept.
            LaunchedEffect(Unit) { if (searching) focus.requestFocus() }
            LaunchedEffect(focused, model.searchText) { if (focused == false && model.searchText.isEmpty()) searching = false }
            // Ink, as on iOS; its text ends on the same 16 dp line as the title above.
            TextButton(onClick = { searching = false; model.perform { model.changeSearch("") } }, Modifier.heightIn(min = 48.dp).padding(end = 2.dp),
                colors = ButtonDefaults.textButtonColors(contentColor = lagom.ink)) { Text("Cancel") }
        } else {
            // The buttons measure first; the filter takes what is left, so large text cannot push them off.
            Box(Modifier.weight(1f)) {
                if (!drafts && model.mailbox == Mailbox.TRASH && model.folder == null) {
                    // In Trash the filter makes way for Empty Trash, as on desktop.
                    var confirm by remember { mutableStateOf(false) }
                    HouseButton(if (model.emptyingTrash) "Emptying…" else "Empty Trash", R.drawable.ic_empty_trash,
                        enabled = model.rows.isNotEmpty() && !model.working) { confirm = true }
                    if (confirm) AlertDialog(onDismissRequest = { confirm = false }, title = { Text("Delete all permanently?") },
                        confirmButton = { TextButton(onClick = { confirm = false; model.perform { model.emptyTrash() } },
                            colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error)) { Text("Delete all") } },
                        dismissButton = { TextButton(onClick = { confirm = false }) { Text("Cancel") } })
                } else if (!drafts) FilterControl(model.unreadOnly) { model.unreadOnly = it }
            }
            Row {
                if (!drafts) LagomIconButton("Search", R.drawable.ic_search) { searching = true }
                LagomIconButton("New message", R.drawable.ic_compose, enabled = model.accounts.isNotEmpty()) {
                    val account = model.accountId ?: model.accounts.first().id
                    model.perform { model.newMessage(account) }
                }
            }
        }
    }
}

/** The Search field, which the list's search button opens: an icon, the text, and Clear once there is some. */
@Composable private fun SearchField(text: String, focus: FocusRequester, modifier: Modifier = Modifier, change: (String) -> Unit) {
    val c = lagom
    // A subtle fill and no outline, as desktop and iOS (Alex 2026-11-15: one look on every platform).
    Row(modifier.height(40.dp).background(c.subtle, RoundedCornerShape(Radius.control))
        .padding(start = 10.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        LagomIcon(R.drawable.ic_search, size = 16.dp)
        Box(Modifier.weight(1f)) {
            val style = MaterialTheme.typography.bodyLarge
            if (text.isEmpty()) Text("Search", Modifier.clearAndSetSemantics {}, color = c.ink2, style = style)
            BasicTextField(text, change, Modifier.fillMaxWidth().focusRequester(focus).semantics { contentDescription = "Search" }, singleLine = true,
                textStyle = style.copy(color = c.ink), cursorBrush = SolidColor(c.accent),
                keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, imeAction = ImeAction.Search))
        }
        if (text.isNotEmpty()) LagomIconButton("Clear search", R.drawable.ic_close, Modifier.size(40.dp)) { change("") }
        else Spacer(Modifier.width(4.dp))
    }
}

/** All / Unread, the desktop list header's filter: icon and label in a small segmented well; the chosen one raised. */
@Composable private fun FilterControl(unreadOnly: Boolean, choose: (Boolean) -> Unit) {
    val c = lagom
    Row(Modifier.height(40.dp).background(c.subtle, RoundedCornerShape(Radius.control)).padding(3.dp)) {
        listOf(false to ("All" to R.drawable.ic_filter_all), true to ("Unread" to R.drawable.ic_unread)).forEach { (unread, choice) ->
            val on = unreadOnly == unread
            // The highlight moves in 140 ms ease-out, as the desktop selection.
            val fill by animateColorAsState(if (on) c.raised else c.subtle, tween(140, easing = EaseOut), label = "filter")
            Row(Modifier.fillMaxHeight().clip(RoundedCornerShape(6.dp)).background(fill)
                .selectable(on, role = Role.Tab) { choose(unread) }.padding(horizontal = 10.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                LagomIcon(choice.second, tint = if (on) c.ink else c.ink2, size = 16.dp)
                // One line: "Unread" broke in two on a narrow phone, as on the folded Duo.
                Text(choice.first, color = if (on) c.ink else c.ink2, style = MaterialTheme.typography.labelLarge, maxLines = 1, softWrap = false)
            }
        }
    }
}

private fun LazyItemScope.rowMotion(animate: Boolean): Modifier =
    if (animate) Modifier.animateItem(fadeInSpec = Fade, placementSpec = landing(IntOffset.VisibilityThreshold), fadeOutSpec = Fade) else Modifier

/** The list's time in the user's 12- or 24-hour setting, desktop `mail::when`. */
@Composable private fun listTime(date: java.time.Instant): String = mailDates().listTime(date)

@Composable private fun mailDates(): MailDates {
    val clock = android.text.format.DateFormat.getTimeFormat(LocalContext.current)
    return MailDates({ clock.format(java.util.Date.from(it)) })
}

/** Mail dates, desktop `mail::when`, `mail::date_group` and `mail::reading_date`, as iOS `DateWords`.
 * `clock` formats a time of day; `format` takes an ICU skeleton. */
internal class MailDates(
    private val clock: (java.time.Instant) -> String,
    private val today: java.time.LocalDate = java.time.LocalDate.now(),
    private val zone: ZoneId = ZoneId.systemDefault(),
    private val format: (java.time.LocalDate, String) -> String = ::localDate,
) {
    private fun day(date: java.time.Instant) = date.atZone(zone).toLocalDate()
    private fun daysAgo(day: java.time.LocalDate) = java.time.temporal.ChronoUnit.DAYS.between(day, today)

    /** The list's time: the clock today, "Yesterday", a weekday this week, "3 Nov" this year, else "Nov 2024". */
    fun listTime(date: java.time.Instant): String {
        val day = day(date)
        val days = daysAgo(day)
        return when {
            days <= 0L -> clock(date)
            days == 1L -> "Yesterday"
            days <= 6L -> format(day, "EEE")
            else -> format(day, if (day.year == today.year) "dMMM" else "MMMyyyy")
        }
    }

    /** The reading pane's date: "Today 09:42", "Fri 09:42", "3 Nov, 09:42". A future date, from a skewed sender clock, shows as a date. */
    fun readingDate(date: java.time.Instant): String {
        val day = day(date)
        val time = clock(date)
        return when (daysAgo(day)) {
            0L -> "Today $time"
            in 1L..6L -> "${listTime(date)} $time"
            else -> "${format(day, if (day.year == today.year) "dMMM" else "dMMMyyyy")}, $time"
        }
    }

    /** The list's group headers: Today, Yesterday, earlier weekdays of this week, then Earlier. */
    fun group(date: java.time.Instant): String {
        val day = day(date)
        val days = daysAgo(day)
        return when {
            days <= 0L -> "Today"
            days == 1L -> "Yesterday"
            days < today.dayOfWeek.value -> format(day, "EEEE") // dayOfWeek is 1 on Monday
            else -> "Earlier"
        }
    }
}

/** Runs of neighbouring items with the same key, in list order, as iOS `grouped`. */
internal fun <T> grouped(items: List<T>, key: (T) -> String): List<Pair<String, List<T>>> {
    val groups = mutableListOf<Pair<String, MutableList<T>>>()
    for (item in items) {
        val title = key(item)
        if (groups.lastOrNull()?.first == title) groups.last().second.add(item) else groups.add(title to mutableListOf(item))
    }
    return groups
}

internal fun swipeStage(dx: Float, rowWidth: Float, reach: Float, canDelete: Boolean): MailAction? = when {
    dx >= reach -> MailAction.Done
    dx < 0f && canDelete && -dx >= rowWidth * 3f / 5f -> MailAction.Delete
    dx <= -reach -> MailAction.Later
    else -> null
}

@OptIn(ExperimentalFoundationApi::class)
@Composable private fun MailListRow(row: MailRow, busy: Boolean, canDelete: Boolean, targets: List<MailFolder>, mark: Color?, onSelect: () -> Unit,
    onAction: (MailAction) -> Unit, onRead: () -> Unit, onMove: (MailFolder) -> Unit, onNewFolder: () -> Unit) {
    // A long press opens what iOS's context menu has: Mark read or unread, and Move to.
    var menu by remember { mutableStateOf(false) }
    var moving by remember { mutableStateOf(false) }
    val readLabel = if (row.unread) "Mark read" else "Mark unread"
    val currentAction by rememberUpdatedState(onAction)
    val currentBusy by rememberUpdatedState(busy)
    // motion.md: a release past 120 dp acts; short of it the row springs back.
    val reach = with(LocalDensity.current) { 120.dp.toPx() }
    val offset = remember { Animatable(0f) }
    var width by remember { mutableFloatStateOf(0f) }
    var settling by remember { mutableStateOf(false) }
    var submitted by remember { mutableStateOf(false) }
    var released by remember { mutableStateOf<MailAction?>(null) }
    val scope = rememberCoroutineScope()
    val haptics = LocalHapticFeedback.current
    val stage = released ?: swipeStage(offset.value, width, reach, canDelete)
    val direction = if (LocalLayoutDirection.current == LayoutDirection.Rtl) -1 else 1
    // A surviving row returns after the action finishes, including a failure before busy was drawn.
    LaunchedEffect(busy, submitted) {
        if (submitted && !busy) {
            withFrameNanos { }
            if (!currentBusy) {
                offset.animateTo(0f, landing())
                submitted = false
                released = null
                settling = false
            }
        }
    }
    val view = RowPresentation(row)
    Box(Modifier.clipToBounds().onSizeChanged { width = it.width.toFloat() }
        .pointerInput(busy, canDelete, reach) {
            if (busy) return@pointerInput
            var dx = 0f
            detectHorizontalDragGestures(
                onDragStart = { dx = offset.value },
                onHorizontalDrag = { change, amount ->
                    if (!currentBusy && !settling) {
                        change.consume()
                        val before = swipeStage(dx, width, reach, canDelete)
                        dx = (dx + amount).coerceIn(-width, width)
                        val after = swipeStage(dx, width, reach, canDelete)
                        if (after != null && after != before) haptics.performHapticFeedback(HapticFeedbackType.GestureThresholdActivate)
                        scope.launch(start = CoroutineStart.UNDISPATCHED) { offset.snapTo(dx) }
                    }
                },
                onDragCancel = {
                    if (!settling) scope.launch { offset.animateTo(0f, landing()) }
                },
                onDragEnd = {
                    if (!settling) {
                        settling = true
                        val action = if (currentBusy) null else swipeStage(dx, width, reach, canDelete)
                        released = action
                        scope.launch {
                            if (action != null) {
                                offset.animateTo(if (dx > 0f) width else -width, tween(150, easing = EaseOut))
                                if (!currentBusy) {
                                    currentAction(action)
                                    submitted = true
                                    return@launch
                                }
                            }
                            offset.animateTo(0f, landing())
                            released = null
                            settling = false
                        }
                    }
                },
            )
        }) {
        val c = lagom
        val fill by animateColorAsState(when (stage) {
            MailAction.Done -> c.done
            MailAction.Later -> c.later
            MailAction.Delete -> c.danger
            null -> c.subtle
        }, tween(100, easing = EaseOut), label = "swipe fill")
        val ink by animateColorAsState(if (stage != null) c.accentInk else c.ink2, tween(100, easing = EaseOut), label = "swipe ink")
        val shown = stage ?: if (offset.value >= 0f) MailAction.Done else MailAction.Later
        Row(Modifier.matchParentSize().background(fill).padding(horizontal = 20.dp).clearAndSetSemantics {},
            horizontalArrangement = if (offset.value >= 0f) Arrangement.Absolute.Left else Arrangement.Absolute.Right,
            verticalAlignment = Alignment.CenterVertically) {
            LagomIcon(when (shown) {
                MailAction.Done -> R.drawable.ic_done
                MailAction.Later -> R.drawable.ic_later
                MailAction.Delete -> R.drawable.ic_trash
            }, tint = ink, size = 20.dp)
            if (stage != null) {
                Spacer(Modifier.width(8.dp))
                Text(when (shown) {
                    MailAction.Done -> "Done"
                    MailAction.Later -> "Later"
                    MailAction.Delete -> "Delete"
                }, color = ink, style = MaterialTheme.typography.labelLarge)
            }
        }
        Box(Modifier.offset { IntOffset((offset.value * direction).roundToInt(), 0) }) {
        Column(Modifier.combinedClickable(onLongClickLabel = "More actions", onLongClick = { if (!busy) menu = true }, onClick = onSelect)
            .semantics {
            stateDescription = if (row.unread) "Unread" else "Read"
            customActions = listOf(CustomAccessibilityAction("Done") { if (!busy) onAction(MailAction.Done); !busy },
                CustomAccessibilityAction("Later") { if (!busy) onAction(MailAction.Later); !busy }) +
                (if (canDelete) listOf(CustomAccessibilityAction("Delete") { if (!busy) onAction(MailAction.Delete); !busy }) else emptyList()) +
                CustomAccessibilityAction(readLabel) { if (!busy) onRead(); !busy } +
                targets.map { folder -> CustomAccessibilityAction("Move to ${folder.path}") { if (!busy) onMove(folder); !busy } } +
                CustomAccessibilityAction("Move to a new folder") { if (!busy) onNewFolder(); !busy }
        }.fillMaxWidth().heightIn(min = RowHeight).background(lagom.bg).padding(start = 10.dp, end = 16.dp, top = 10.dp, bottom = 10.dp)) {
            // The desktop list row: unread dot, account bar, sender and time; subject and a two-line snippet under them.
            val c = lagom
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Box(Modifier.size(7.dp).background(if (row.unread) c.accent else Color.Transparent, CircleShape))
                mark?.let { Box(Modifier.size(3.dp, 16.dp).background(it, RoundedCornerShape(1.5.dp))) }
                Text(view.sender, Modifier.weight(1f), fontWeight = if (row.unread) FontWeight.SemiBold else FontWeight.Normal,
                    maxLines = 1, overflow = TextOverflow.Ellipsis)
                if (row.messageCount > 1u) Text(row.messageCount.toString(), Modifier.background(c.subtle, RoundedCornerShape(50)).padding(horizontal = 6.dp)
                    .semantics { contentDescription = "${row.messageCount} messages" }, style = MaterialTheme.typography.bodySmall)
                if (row.hasAttachments) LagomIcon(R.drawable.ic_attachment, size = 15.dp, label = "Attachment")
                Text(listTime(view.date), color = c.ink2, style = MaterialTheme.typography.bodySmall.copy(fontFeatureSettings = "tnum"))
            }
            Column(Modifier.padding(start = 13.dp, top = 2.dp)) {
                Text(view.subject, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(row.preview, color = c.ink2, style = MaterialTheme.typography.bodyMedium, maxLines = 2, minLines = 2, overflow = TextOverflow.Ellipsis)
            }
        }
        DropdownMenu(menu || moving, { menu = false; moving = false }) {
            if (moving) {
                targets.forEach { folder -> DropdownMenuItem(text = { Text(folder.path) }, onClick = { moving = false; onMove(folder) }) }
                // Desktop's Move to offers "New folder" too; the message then moves there.
                DropdownMenuItem(text = { Text("New folder…") }, enabled = !busy, onClick = { moving = false; onNewFolder() })
            } else {
                DropdownMenuItem(text = { Text(readLabel) }, onClick = { menu = false; onRead() })
                DropdownMenuItem(text = { Text("Move to…") }, onClick = { menu = false; moving = true })
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
            if (!single) EmptyState(R.drawable.ic_mail, "No message selected")
            return@AnimatedContent
        }
        val back = rememberBackPreview(enabled = shown.id == model.selectedId && model.draft == null && model.accountForm == null) {
            model.clearSelection(); false
        }
        val close = if (single) model::clearSelection else null
        // Surface also keeps touches from reaching the list underneath.
        Surface(Modifier.fillMaxSize().backPreview(back), color = lagom.bg) {
            Column(Modifier.fillMaxSize()) {
                val one = shown.messages.singleOrNull()
                // A conversation has one bar, over all its messages, as on Mac and iOS: its actions take the grouped row.
                if (one == null && shown.messages.isNotEmpty()) {
                    val row = model.rows.firstOrNull { shown.id in it.messageIds }
                    val unread = row?.unread ?: shown.messages.any { it.row.unread }
                    MessageBar(model, shown.id, shown.messages.last().row.account, unread, conversation = true, close) {
                        if (row != null) model.setRead(row) else shown.messages.forEach { model.setRead(it.row.id, unread) }
                    }
                } else if (one == null) TopBar(Modifier.statusBarsPadding(), start = 4.dp) { close?.let { LagomIconButton("Back", R.drawable.ic_chevron_left, onClick = it) } }
                Box(Modifier.weight(1f)) {
                    when {
                        one != null -> MessageScreen(model, one, close)
                        shown.messages.isEmpty() -> MessagePlaceholder(model.rows.firstOrNull { it.id == shown.id })
                        else -> ConversationScreen(model, shown.messages)
                    }
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
        id in model.unsubscribedIds -> Text("Unsubscribed", color = lagom.ink2, style = quiet)
        model.unsubscribingId == id -> Text("Unsubscribing…", color = lagom.ink2, style = quiet)
        else -> Text("Unsubscribe from ${offer.list}", Modifier.heightIn(min = 48.dp).clickable(role = Role.Button) { confirm = true }
            .wrapContentHeight(), color = lagom.ink2, style = quiet, maxLines = 2)
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

/** motion.md "Loading": nothing for the first 150 ms, then the row's preview stands in for the body, under a bar as tall
 *  as the message's own, so nothing moves when the body lands. */
@Composable private fun MessagePlaceholder(row: MailRow?) {
    TopBar(Modifier.statusBarsPadding()) {}
    if (!afterWait(true)) return
    Column(Modifier.fillMaxSize().padding(top = BarHeight).padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        if (row == null) { Text("Loading message…", color = lagom.ink2); return@Column }
        val view = RowPresentation(row)
        Text(view.subject, style = MaterialTheme.typography.headlineSmall)
        Text(view.sender, fontWeight = FontWeight.SemiBold)
        Text(row.preview, color = lagom.ink2)
    }
}

@Composable private fun ConversationScreen(model: MailModel, messages: List<MailMessage>) {
    LazyColumn(Modifier.fillMaxSize()) {
        items(messages, key = { it.row.id }) { message ->
            val row = message.row
            val view = RowPresentation(row)
            val expanded = row.id in model.expandedIds
            Row(Modifier.fillMaxWidth().heightIn(min = 48.dp)
                .clickable(role = Role.Button, onClickLabel = "${if (expanded) "Collapse" else "Expand"} message from ${view.sender}") {
                    model.perform { model.toggleMessage(row.id) }
                }.padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                LagomIcon(if (expanded) R.drawable.ic_chevron else R.drawable.ic_chevron_right, size = 16.dp)
                Text(view.sender, Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis, fontWeight = if (row.unread) FontWeight.SemiBold else FontWeight.Normal)
                Text(row.preview, Modifier.weight(1f), color = lagom.ink2, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodyMedium)
                Text(listTime(view.date), color = lagom.ink2, style = MaterialTheme.typography.bodySmall)
            }
            // shortcut: like desktop, expanded bodies scroll independently; measure HTML for continuous scrolling later.
            if (expanded) Box(Modifier.height(560.dp)) { MessageScreen(model, message, close = null, bar = false) }
            HorizontalDivider()
        }
    }
}

/** The reading pane's bar: Back, Reply, Later, More and Done filled, the same set and order as Mac and iOS. With
 *  `conversation`, the actions take every message of the grouped row. */
@Composable private fun MessageBar(model: MailModel, id: Long, account: String, unread: Boolean, conversation: Boolean,
    close: (() -> Unit)?, markRead: suspend () -> Unit) {
    TopBar(Modifier.statusBarsPadding(), start = if (close != null) 4.dp else 12.dp, end = 12.dp) {
        close?.let { LagomIconButton("Back", R.drawable.ic_chevron_left, onClick = it) }
        HouseButton("Reply", R.drawable.ic_reply, iconOnly = true, enabled = !model.working) { model.perform { model.reply(id) } }
        Box {
            var choices by remember { mutableStateOf<List<LaterChoice>>(emptyList()) }
            // One tap is tomorrow, as desktop's L; a long press offers the other times.
            HouseButton("Later", R.drawable.ic_later, iconOnly = true, enabled = !model.working,
                onLongClickLabel = "Choose a time", onLongClick = { choices = model.core?.laterChoices().orEmpty() }) {
                model.perform { model.act(MailAction.Later, id, conversation = conversation) }
            }
            DropdownMenu(expanded = choices.isNotEmpty(), onDismissRequest = { choices = emptyList() }) {
                choices.forEachIndexed { ix, choice ->
                    DropdownMenuItem(text = { Text(choice.label) }, onClick = {
                        choices = emptyList()
                        // The time at the tap, as on iOS: an evening chosen after it began would be in the past.
                        val until = model.core?.laterChoices()?.getOrNull(ix)?.untilUnix ?: choice.untilUnix
                        model.perform { model.act(MailAction.Later, id, conversation = conversation, until = until) }
                    })
                }
            }
        }
        val targets = model.moveTargets(account)
        Box {
            var open by remember { mutableStateOf(false) }
            var moving by remember { mutableStateOf(false) }
            HouseButton("More", R.drawable.ic_more, iconOnly = true, enabled = !model.working) { open = true }
            DropdownMenu(expanded = open || moving, onDismissRequest = { open = false; moving = false }) {
                if (moving) {
                    targets.forEach { folder ->
                        DropdownMenuItem(text = { Text(folder.path) }, onClick = {
                            moving = false
                            model.perform { model.move(id, folder, conversation = conversation) }
                        })
                    }
                    DropdownMenuItem(text = { Text("New folder…") }, enabled = !model.working, onClick = {
                        moving = false
                        model.newFolder = NewFolder(account, id, conversation = conversation)
                    })
                } else {
                    // The same five, in the same order, on every platform.
                    @Composable fun item(label: String, @DrawableRes icon: Int, run: () -> Unit) =
                        DropdownMenuItem(text = { Text(label) }, leadingIcon = { LagomIcon(icon) }, onClick = { open = false; run() })
                    item("Reply all", R.drawable.ic_reply_all) { model.perform { model.reply(id, ComposeKind.REPLY_ALL) } }
                    item("Forward", R.drawable.ic_forward) { model.perform { model.reply(id, ComposeKind.FORWARD) } }
                    item("Move", R.drawable.ic_move) { moving = true }
                    if (model.mailbox != Mailbox.TRASH) item("Delete", R.drawable.ic_trash) {
                        model.perform { model.act(MailAction.Delete, id, conversation = conversation) }
                    }
                    item(if (unread) "Mark read" else "Mark unread", if (unread) R.drawable.ic_read else R.drawable.ic_unread) {
                        model.perform { markRead() }
                    }
                }
            }
        }
        Spacer(Modifier.weight(1f))
        HouseButton("Done", R.drawable.ic_done, primary = true, enabled = !model.working) {
            model.perform { model.act(MailAction.Done, id, conversation = conversation) }
        }
    }
}

/** The reading pane: a bar with Back, Reply, Later, More and Done filled, the same set and order as Mac and iOS; then the
 *  subject, the sender with the message menu, the body, attachments, and Reply. */
@Composable private fun MessageScreen(model: MailModel, message: MailMessage, close: (() -> Unit)?, bar: Boolean = true) {
    var plain by remember(message.row.id) { mutableStateOf(false) }
    val context = LocalContext.current
    val c = lagom
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
    val id = message.row.id
    Column(Modifier.fillMaxSize()) {
        // In a conversation the bar sits once above all messages; each card keeps its own menu.
        if (bar) MessageBar(model, id, message.row.account, message.row.unread, conversation = false, close) { model.setRead(id, message.row.unread) }
        Column(Modifier.weight(1f).padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            SelectionContainer {
                Text(message.row.subject.ifEmpty { "(No subject)" }, Modifier.padding(top = 16.dp).semantics { heading() },
                    style = MaterialTheme.typography.headlineSmall, maxLines = 3, overflow = TextOverflow.Ellipsis)
            }
            Header(model, message, plain) { plain = !plain }
            message.unsubscribe?.let { UnsubscribeLine(model, id, it) }
            if (message.htmlBody != null && !plain && id !in model.remoteImagesIds) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    HouseButton("Load remote images", R.drawable.ic_image) { model.loadRemoteImages(id) }
                    Text("Remote images are blocked", color = c.ink2, style = MaterialTheme.typography.bodySmall)
                }
            }
            Box(Modifier.weight(1f)) {
                if (!plain && message.htmlBody != null) MailHtml(id, message.htmlBody!!, id in model.remoteImagesIds)
                else SelectionContainer {
                    Text(message.textBody, Modifier.fillMaxSize().verticalScroll(rememberScrollState()),
                        style = MaterialTheme.typography.bodyLarge.copy(fontSize = TextSize.body, lineHeight = TextSize.body * ReadingLeading))
                }
            }
            if (message.attachments.isNotEmpty()) {
                LazyRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    items(message.attachments, key = { it.section }) { part ->
                        val downloading = (id to part.section) in model.downloadingAttachments
                        val name = safeAttachmentName(part.filename)
                        Row(Modifier.heightIn(min = 48.dp).clip(RoundedCornerShape(Radius.attachment)).background(c.subtle)
                            .clickable(enabled = !downloading, onClickLabel = "Open", role = Role.Button) { openAttachment(part) }
                            .padding(start = 14.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                            if (afterWait(downloading)) CircularProgressIndicator(Modifier.size(16.dp), color = c.ink2, strokeWidth = 2.dp)
                            else LagomIcon(R.drawable.ic_attachment, size = 16.dp)
                            Text(name, Modifier.widthIn(max = 180.dp), style = MaterialTheme.typography.bodyMedium.copy(fontWeight = FontWeight.Medium),
                                maxLines = 1, overflow = TextOverflow.MiddleEllipsis)
                            Text(android.text.format.Formatter.formatShortFileSize(context, part.size.toLong()), color = c.ink2,
                                style = MaterialTheme.typography.bodySmall)
                            LagomIconButton("Share $name", R.drawable.ic_external, enabled = !downloading) { openAttachment(part, share = true) }
                        }
                    }
                }
            }
            // The desktop reply box: one quiet line that opens Reply.
            Row(Modifier.fillMaxWidth().padding(bottom = 12.dp).heightIn(min = 48.dp).clip(RoundedCornerShape(Radius.control))
                .background(c.subtle)
                .clickable(enabled = !model.working, role = Role.Button) { model.perform { model.reply(id) } }.padding(horizontal = 12.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                LagomIcon(R.drawable.ic_reply)
                Text("Reply to ${RowPresentation(message.row).sender}", color = c.ink2, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
    }
}

/** Initials, sender, address and date, with the desktop's message menu: Reply all, Forward, Mark read, plain text. */
@Composable private fun Header(model: MailModel, message: MailMessage, plain: Boolean, togglePlain: () -> Unit) {
    val row = message.row
    val c = lagom
    val sender = RowPresentation(row).sender
    val initials = sender.split(" ").filter { it.isNotEmpty() }.take(2).map { it.first().uppercaseChar() }.joinToString("")
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Box(Modifier.size(36.dp).background(c.subtle, CircleShape).clearAndSetSemantics {},
            contentAlignment = Alignment.Center) {
            Text(initials, style = MaterialTheme.typography.bodySmall.copy(fontWeight = FontWeight.SemiBold), maxLines = 1)
        }
        // The address wraps, so a look-alike sender's domain stays visible.
        SelectionContainer(Modifier.weight(1f)) {
            Column(verticalArrangement = Arrangement.spacedBy(1.dp)) {
                Text(sender, fontWeight = FontWeight.SemiBold, maxLines = 2)
                Text(row.senderAddress, color = c.ink2, style = MaterialTheme.typography.bodySmall)
                val date = RowPresentation(row).date
                Text(mailDates().readingDate(date), color = c.ink2, style = MaterialTheme.typography.bodySmall)
            }
        }
        Box {
            var open by remember { mutableStateOf(false) }
            LagomIconButton("Message actions", R.drawable.ic_more) { open = true }
            DropdownMenu(open, { open = false }) {
                val readLabel = if (row.unread) "Mark read" else "Mark unread"
                DropdownMenuItem(text = { Text("Reply all") }, enabled = !model.working, onClick = { open = false; model.perform { model.reply(row.id, ComposeKind.REPLY_ALL) } })
                DropdownMenuItem(text = { Text("Forward") }, enabled = !model.working, onClick = { open = false; model.perform { model.reply(row.id, ComposeKind.FORWARD) } })
                DropdownMenuItem(text = { Text(readLabel) }, enabled = !model.working, onClick = { open = false; model.perform { model.setRead(row.id, row.unread) } })
                if (message.htmlBody != null) DropdownMenuItem(text = { Text(if (plain) "Show formatted" else "Show plain text") },
                    onClick = { open = false; togglePlain() })
            }
        }
    }
}
