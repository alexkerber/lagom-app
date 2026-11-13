package app.lagommail.android

import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.Intent
import android.webkit.MimeTypeMap
import androidx.core.content.FileProvider
import androidx.compose.ui.platform.LocalContext
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.*
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import app.lagommail.core.*
import kotlinx.coroutines.launch
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle

@OptIn(ExperimentalMaterial3Api::class)
@Composable internal fun MailScreen(model: MailModel, twoPane: Boolean) {
    val adding = model.accountForm != null
    var menu by remember { mutableStateOf(false) }
    var removing by remember { mutableStateOf<MailAccount?>(null) }
    var retrying by remember { mutableStateOf<SendFailure?>(null) }
    BackHandler(model.selectedId != null && model.draft == null && !adding) { model.clearSelection() }
    when {
        adding -> AddAccountScreen(model) { model.accountForm = null }
        model.draft != null -> ComposeScreen(model)
        else -> Scaffold(topBar = {
            TopAppBar(title = { Text(if (model.showingDrafts) "Drafts" else if (model.accounts.isEmpty()) "lagom" else model.mailbox.label()) }, navigationIcon = {
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
                            DropdownMenuItem(text = { Text("Remove ${account.email}") }, onClick = { menu = false; removing = account })
                        }
                    }
                }
            }, actions = {
                TextButton(enabled = model.accounts.isNotEmpty(), onClick = {
                    val account = model.accountId ?: model.accounts.first().id
                    model.draft = ComposeDraft(account, id = draftMessageId(model.accounts.firstOrNull { it.id == account }?.email.orEmpty()))
                }) { Text("Compose") }
            })
        }) { padding ->
            Column(Modifier.fillMaxSize().padding(padding)) {
                model.notice?.let { notice ->
                    Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                        Text(notice, Modifier.weight(1f))
                        if (model.undoSendId != null) {
                            TextButton(enabled = !model.sending, onClick = { model.perform { model.undoSend() } },
                                modifier = Modifier.sizeIn(minWidth = 48.dp, minHeight = 48.dp)
                                    .semantics { contentDescription = "Undo send" }) { Text("Undo") }
                        } else { TextButton(onClick = { model.notice = null }) { Text("Dismiss") } }
                    }
                }
                if (model.loading) LinearProgressIndicator(Modifier.fillMaxWidth())
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
                        if (drafts.isEmpty()) item { Text("No drafts", Modifier.padding(24.dp)) }
                        items(drafts, key = { it.id }) { draft ->
                            ListItem(headlineContent = { Text(draft.subject.ifEmpty { "(No subject)" }) },
                                supportingContent = { Column {
                                    Text(draft.to)
                                    Text(model.accounts.find { it.id == draft.account }?.email.orEmpty())
                                    if (draft.updatedUnix > 0) Text(DateTimeFormatter.ofLocalizedDateTime(FormatStyle.SHORT)
                                        .withZone(ZoneId.systemDefault()).format(java.time.Instant.ofEpochSecond(draft.updatedUnix)))
                                } }, modifier = Modifier.heightIn(min = 48.dp).clickable(onClickLabel = "Edit draft") { model.perform { model.openDraft(draft) } })
                            HorizontalDivider()
                        }
                    }
                } else Row(Modifier.weight(1f)) {
                    if (twoPane || model.selectedId == null) {
                        Box(if (twoPane) Modifier.weight(0.42f) else Modifier.fillMaxSize()) {
                            Inbox(model, onRetry = { retrying = it })
                        }
                    }
                    if (twoPane) VerticalDivider()
                    if (twoPane || model.selectedId != null) {
                        Box(if (twoPane) Modifier.weight(0.58f) else Modifier.fillMaxSize()) {
                            val message = model.message
                            if (message != null) ConversationScreen(model, !twoPane)
                            else Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { Text(if (model.selectedId == null) "Select a message" else "Loading message…") }
                        }
                    }
                }
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
            FilterChip(!model.unreadOnly, { model.unreadOnly = false }, label = { Text("All") },
                modifier = Modifier.sizeIn(minWidth = 48.dp, minHeight = 48.dp).semantics { contentDescription = "All" })
            FilterChip(model.unreadOnly, { model.unreadOnly = true }, label = { Text("Unread") },
                modifier = Modifier.sizeIn(minWidth = 48.dp, minHeight = 48.dp).semantics { contentDescription = "Unread" })
        }
        PullToRefreshBox(isRefreshing = model.syncing, onRefresh = { model.perform { model.sync() } }, modifier = Modifier.weight(1f)) {
            LazyColumn(Modifier.fillMaxSize()) {
                if (model.visibleRows.isEmpty() && !model.syncing) item {
                    Text(if (model.unreadOnly && model.rows.isNotEmpty()) "No unread mail" else "No mail here. Pull to refresh.", Modifier.padding(24.dp))
                }
                items(model.failures, key = { "failure-${it.id}" }) { failure ->
                    ListItem(headlineContent = { Text("Outbox: ${failure.subject}") }, supportingContent = { Text(failure.message) },
                        modifier = Modifier.clickable { onRetry(failure) })
                }
                items(model.visibleRows, key = { it.id }) { row ->
                    MailListRow(row, model.working, model.mailbox != Mailbox.TRASH, onSelect = { model.perform { model.select(row.id) } },
                        onAction = { model.perform { model.act(it, row.id) } })
                    HorizontalDivider()
                }
                if (model.hasMore) item {
                    TextButton(onClick = { model.perform { model.reload(more = true) } },
                        modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp).semantics { contentDescription = "Load more" }) { Text("Load more") }
                }
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable private fun MailListRow(row: MailRow, busy: Boolean, canDelete: Boolean, onSelect: () -> Unit, onAction: (MailAction) -> Unit) {
    val currentAction by rememberUpdatedState(onAction)
    val currentBusy by rememberUpdatedState(busy)
    val state = rememberSwipeToDismissBoxState(confirmValueChange = { value ->
        if (!currentBusy) when (value) {
            SwipeToDismissBoxValue.StartToEnd -> currentAction(MailAction.Done)
            else -> Unit
        }
        value == SwipeToDismissBoxValue.Settled || (!currentBusy && value == SwipeToDismissBoxValue.EndToStart)
    })
    val scope = rememberCoroutineScope()
    val revealed = state.currentValue == SwipeToDismissBoxValue.EndToStart
    val view = RowPresentation(row)
    SwipeToDismissBox(state = state, gesturesEnabled = !busy, backgroundContent = {
        val done = state.dismissDirection == SwipeToDismissBoxValue.StartToEnd
        if (done) Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.tertiary).padding(20.dp), contentAlignment = Alignment.CenterStart) {
            Text("Done", color = MaterialTheme.colorScheme.onTertiary)
        } else Row(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.surfaceVariant)
            .then(if (revealed) Modifier else Modifier.clearAndSetSemantics {}),
            horizontalArrangement = Arrangement.End, verticalAlignment = Alignment.CenterVertically) {
            TextButton(enabled = !busy, onClick = { scope.launch { state.reset(); currentAction(MailAction.Later) } },
                modifier = Modifier.sizeIn(minWidth = 48.dp, minHeight = 48.dp).semantics { contentDescription = "Later" }) { Text("Later") }
            if (canDelete) TextButton(enabled = !busy, onClick = { scope.launch { state.reset(); currentAction(MailAction.Delete) } },
                colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error),
                modifier = Modifier.sizeIn(minWidth = 48.dp, minHeight = 48.dp).semantics { contentDescription = "Delete" }) { Text("Delete") }
            TextButton(onClick = { scope.launch { state.reset() } },
                modifier = Modifier.sizeIn(minWidth = 48.dp, minHeight = 48.dp).semantics { contentDescription = "Cancel" }) { Text("Cancel") }
        }
    }) {
        ListItem(modifier = Modifier.clickable(onClick = onSelect).then(if (revealed) Modifier.clearAndSetSemantics {} else Modifier.semantics {
            stateDescription = if (row.unread) "Unread" else "Read"
            customActions = listOf(CustomAccessibilityAction("Done") { if (!busy) onAction(MailAction.Done); !busy },
                CustomAccessibilityAction("Later") { if (!busy) onAction(MailAction.Later); !busy }) +
                if (canDelete) listOf(CustomAccessibilityAction("Delete") { if (!busy) onAction(MailAction.Delete); !busy }) else emptyList()
        }), headlineContent = { Text(view.sender, fontWeight = if (row.unread) FontWeight.Bold else FontWeight.Normal, maxLines = 1) },
            supportingContent = { Column { Text(view.subject, maxLines = 1); Text(row.preview, maxLines = 2) } },
            trailingContent = { Column(horizontalAlignment = Alignment.End) {
                Text(DateTimeFormatter.ofLocalizedDate(FormatStyle.SHORT).withZone(ZoneId.systemDefault()).format(view.date), style = MaterialTheme.typography.labelSmall)
                if (row.messageCount > 1u) Badge {
                    Text(row.messageCount.toString(), Modifier.semantics { contentDescription = "${row.messageCount} messages" })
                }
                if (row.hasAttachments) Text("Attachment", style = MaterialTheme.typography.labelSmall)
            } })
    }
}

@Composable private fun ConversationScreen(model: MailModel, back: Boolean) {
    Column(Modifier.fillMaxSize()) {
        if (back) TextButton(onClick = model::clearSelection) { Text("Back") }
        // Most mail is a single message: keep the full-height reading view for it.
        model.messages.singleOrNull()?.let { Box(Modifier.weight(1f)) { MessageScreen(model, it) }; return@Column }
        LazyColumn(Modifier.weight(1f)) {
            items(model.messages, key = { it.row.id }) { message ->
                val row = message.row
                val view = RowPresentation(row)
                val expanded = row.id in model.expandedIds
                Row(Modifier.fillMaxWidth().heightIn(min = 48.dp)
                    .clickable(role = Role.Button, onClickLabel = "${if (expanded) "Collapse" else "Expand"} message from ${view.sender}") {
                        model.perform { model.toggleMessage(row.id) }
                    }.padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(if (expanded) "⌄" else "›")
                    Text(view.sender, Modifier.weight(1f), maxLines = 1, fontWeight = if (row.unread) FontWeight.Bold else FontWeight.Normal)
                    Text(row.preview, Modifier.weight(1f), maxLines = 1, style = MaterialTheme.typography.bodySmall)
                    Text(DateTimeFormatter.ofLocalizedDate(FormatStyle.SHORT).withZone(ZoneId.systemDefault()).format(view.date), style = MaterialTheme.typography.labelSmall)
                }
                // shortcut: like desktop, expanded bodies scroll independently; measure HTML for continuous scrolling later.
                if (expanded) Box(Modifier.height(560.dp)) { MessageScreen(model, message) }
                HorizontalDivider()
            }
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
            val mime = part.mimeType.takeIf { it.isNotBlank() && it != "application/octet-stream" }
                ?: MimeTypeMap.getSingleton().getMimeTypeFromExtension(file.extension.lowercase(java.util.Locale.ROOT))
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
                            if (downloading) CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
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
            TextButton(enabled = !model.working, onClick = { model.perform { model.act(MailAction.Delete, message.row.id, conversation = false) } }) { Text("Delete") }
            Button(enabled = !model.working, onClick = { model.perform { model.act(MailAction.Done, message.row.id, conversation = false) } }) { Text("Done") }
        }
    }
}
