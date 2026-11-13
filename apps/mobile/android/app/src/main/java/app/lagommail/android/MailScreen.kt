package app.lagommail.android

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.*
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.lagommail.core.*
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
                model.notice?.let { notice -> TextButton(onClick = { model.notice = null }) { Text(notice) } }
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
                            if (message != null) MessageScreen(model, message, !twoPane)
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
    PullToRefreshBox(isRefreshing = model.syncing, onRefresh = { model.perform { model.sync() } }) {
        LazyColumn(Modifier.fillMaxSize()) {
            if (model.rows.isEmpty() && !model.syncing) item {
                Text("No mail here. Pull to refresh.", Modifier.padding(24.dp))
            }
            items(model.failures, key = { "failure-${it.id}" }) { failure ->
                ListItem(headlineContent = { Text("Outbox: ${failure.subject}") }, supportingContent = { Text(failure.message) },
                    modifier = Modifier.clickable { onRetry(failure) })
            }
            items(model.rows, key = { it.id }) { row ->
                MailListRow(row, model.working, onSelect = { model.perform { model.select(row.id) } },
                    onAction = { model.perform { model.act(it, row.id) } })
                HorizontalDivider()
            }
            if (model.hasMore) item {
                TextButton(onClick = { model.perform { model.reload(more = true) } }, modifier = Modifier.fillMaxWidth()) { Text("Load more") }
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable private fun MailListRow(row: MailRow, busy: Boolean, onSelect: () -> Unit, onAction: (MailAction) -> Unit) {
    val currentAction by rememberUpdatedState(onAction)
    val currentBusy by rememberUpdatedState(busy)
    val state = rememberSwipeToDismissBoxState(confirmValueChange = { value ->
        if (!currentBusy) when (value) {
            SwipeToDismissBoxValue.StartToEnd -> currentAction(MailAction.Done)
            SwipeToDismissBoxValue.EndToStart -> currentAction(MailAction.Later)
            else -> Unit
        }
        false
    })
    val view = RowPresentation(row)
    SwipeToDismissBox(state = state, gesturesEnabled = !busy, backgroundContent = {
        val done = state.dismissDirection == SwipeToDismissBoxValue.StartToEnd
        Box(Modifier.fillMaxSize().background(if (done) MaterialTheme.colorScheme.tertiary else MaterialTheme.colorScheme.secondary).padding(20.dp),
            contentAlignment = if (done) Alignment.CenterStart else Alignment.CenterEnd) {
            Text(if (done) "Done" else "Later", color = if (done) MaterialTheme.colorScheme.onTertiary else MaterialTheme.colorScheme.onSecondary)
        }
    }) {
        ListItem(modifier = Modifier.clickable(onClick = onSelect).semantics {
            stateDescription = if (row.unread) "Unread" else "Read"
            customActions = listOf(CustomAccessibilityAction("Done") { if (!busy) onAction(MailAction.Done); !busy },
                CustomAccessibilityAction("Later") { if (!busy) onAction(MailAction.Later); !busy })
        }, headlineContent = { Text(view.sender, fontWeight = if (row.unread) FontWeight.Bold else FontWeight.Normal, maxLines = 1) },
            supportingContent = { Column { Text(view.subject, maxLines = 1); Text(row.preview, maxLines = 2) } },
            trailingContent = { Column(horizontalAlignment = Alignment.End) {
                Text(DateTimeFormatter.ofLocalizedDate(FormatStyle.SHORT).withZone(ZoneId.systemDefault()).format(view.date), style = MaterialTheme.typography.labelSmall)
                if (row.hasAttachments) Text("Attachment", style = MaterialTheme.typography.labelSmall)
            } })
    }
}

@Composable private fun MessageScreen(model: MailModel, message: MailMessage, back: Boolean) {
    var plain by remember(message.row.id) { mutableStateOf(false) }
    Column(Modifier.fillMaxSize().padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        if (back) TextButton(onClick = model::clearSelection) { Text("Back") }
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
                Checkbox(plain, { plain = it }); Text("Plain text · remote images blocked")
            }
        }
        Box(Modifier.weight(1f)) {
            if (!plain && message.htmlBody != null) MailHtml(message.row.id, message.htmlBody!!)
            else SelectionContainer { Text(message.textBody, Modifier.fillMaxSize().verticalScroll(rememberScrollState())) }
        }
        // shortcut: attachment metadata only, add transfers when mobile file handling is implemented.
        if (message.attachments.isNotEmpty()) {
            LazyColumn(Modifier.heightIn(max = 90.dp)) {
                items(message.attachments) { Text("${it.filename} · ${it.size} bytes", style = MaterialTheme.typography.bodySmall) }
            }
        }
        FlowRow(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
            TextButton(enabled = !model.working, onClick = { model.perform { model.reply(message.row.id) } }) { Text("Reply") }
            TextButton(enabled = !model.working, onClick = { model.perform { model.act(MailAction.Later, message.row.id) } }) { Text("Later") }
            TextButton(enabled = !model.working, onClick = { model.perform { model.act(MailAction.Delete, message.row.id) } }) { Text("Delete") }
            Button(enabled = !model.working, onClick = { model.perform { model.act(MailAction.Done, message.row.id) } }) { Text("Done") }
        }
    }
}
