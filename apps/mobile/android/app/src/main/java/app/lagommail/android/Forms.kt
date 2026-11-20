package app.lagommail.android

import android.content.ContentResolver
import android.net.Uri
import android.provider.OpenableColumns
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp
import app.lagommail.core.*
import kotlinx.coroutines.CancellationException

@OptIn(ExperimentalMaterial3Api::class)
@Composable internal fun ComposeScreen(model: MailModel, draft: ComposeDraft) {
    // While the screen animates out the model holds no draft; an edit then must not reopen it.
    val live = model.draft?.id == draft.id
    fun edit(next: ComposeDraft) { if (model.draft?.id == next.id) model.draft = next }
    var cancelling by remember(draft.id) { mutableStateOf(false) }
    var contactsAccount by remember(draft.id) { mutableStateOf<String?>(null) }
    var contactsError by remember(draft.id) { mutableStateOf<String?>(null) }
    LaunchedEffect(draft.id, draft.account) {
        contactsAccount = null
        contactsError = null
        try {
            model.core?.refreshContacts(draft.account)
            contactsAccount = draft.account
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (_: Exception) { contactsError = "Could not load contacts. Enter an address to continue." }
    }
    val resolver = LocalContext.current.contentResolver
    val add = { uris: List<Uri> -> if (uris.isNotEmpty()) model.perform { model.attach { uris.map { resolver.attachment(it) } } } }
    val files = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { add(it) }
    val media = rememberLauncherForActivityResult(ActivityResultContracts.PickMultipleVisualMedia()) { add(it) }
    var attachMenu by remember { mutableStateOf(false) }
    // Back keeps the draft, as closing the sheet does on iOS; Cancel still asks.
    // Enabled on `live` alone: disabling the handler cancels its running close, which could leave the screen shrunk.
    val back = rememberBackPreview(enabled = live) {
        model.perform { model.closeCompose(discard = false) }.join()
        model.draft != null
    }
    // While a send or save runs, back does nothing rather than leaving the app. Added last, so it wins.
    BackHandler(live && model.sending) {}
    if (cancelling) {
        AlertDialog(onDismissRequest = { cancelling = false }, title = { Text("Save draft?") },
            confirmButton = { TextButton(enabled = !model.sending, modifier = Modifier.heightIn(min = 48.dp), onClick = {
                cancelling = false; model.perform { model.closeCompose(discard = false) }
            }) { Text("Keep draft") } },
            dismissButton = { TextButton(enabled = !model.sending, modifier = Modifier.heightIn(min = 48.dp), onClick = {
                cancelling = false; model.perform { model.closeCompose(discard = true) }
            }) { Text("Discard") } })
    }
    // The desktop compose sheet: a bar with Cancel, the title, Attach and Send; then From, To, Cc, Subject lines and the body.
    Scaffold(Modifier.backPreview(back), containerColor = lagom.bg, topBar = {
        TopBar(Modifier.statusBarsPadding(), start = 4.dp, end = 12.dp) {
            LagomIconButton("Cancel", R.drawable.ic_close, enabled = !model.sending) { cancelling = true }
            BarTitle(draft.title, Modifier.weight(1f))
            Box {
                LagomIconButton("Attach", R.drawable.ic_attachment, enabled = !model.sending && !model.attaching) { attachMenu = true }
                DropdownMenu(attachMenu, { attachMenu = false }) {
                    DropdownMenuItem(text = { Text("Files") }, onClick = { attachMenu = false; files.launch(arrayOf("*/*")) })
                    DropdownMenuItem(text = { Text("Photos") }, onClick = {
                        attachMenu = false; media.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageAndVideo))
                    })
                }
            }
            HouseButton(if (model.sending) "Queuing…" else "Send", R.drawable.ic_send, primary = true,
                enabled = !model.sending && !model.attaching && (draft.to.isNotBlank() || draft.cc.isNotBlank() || draft.bcc.isNotBlank()) && draft.account.isNotBlank(),
                onClick = { model.perform { model.send() } })
        }
    }) { padding ->
        Column(Modifier.fillMaxSize().padding(padding).imePadding().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp)) {
            var fromMenu by remember { mutableStateOf(false) }
            Box {
                // Changing sender mid-read would give the draft a new id and drop the pending files.
                val fixed = draft.isReply || draft.restored
                FieldRow("From", Modifier.clickable(enabled = !model.sending && !model.attaching && !fixed, role = Role.Button) { fromMenu = true }
                    .semantics(mergeDescendants = true) {}) {
                    Text(model.accounts.find { it.id == draft.account }?.email.orEmpty(), maxLines = 1, overflow = TextOverflow.Ellipsis)
                    if (!fixed) LagomIcon(R.drawable.ic_chevron, Modifier.padding(start = 4.dp), size = 14.dp)
                }
                DropdownMenu(fromMenu, { fromMenu = false }) {
                    model.accounts.forEach { account ->
                        DropdownMenuItem(text = { Text(account.email) }, onClick = { fromMenu = false; model.perform { model.changeComposeAccount(account.id) } })
                    }
                }
            }
            RecipientField("To", draft.to, !model.sending, model.core, draft.account, contactsAccount == draft.account) { edit(draft.copy(to = it)) }
            RecipientField("Cc", draft.cc, !model.sending, model.core, draft.account, contactsAccount == draft.account) { edit(draft.copy(cc = it)) }
            if (draft.showBcc) RecipientField("Bcc", draft.bcc, !model.sending, model.core, draft.account, contactsAccount == draft.account) { edit(draft.copy(bcc = it)) }
            contactsError?.let { Text(it, Modifier.padding(vertical = 6.dp), color = lagom.danger, style = MaterialTheme.typography.bodySmall) }
            FieldRow("Subject") {
                LineInput(draft.subject, { edit(draft.copy(subject = it)) }, !model.sending, "Subject",
                    MaterialTheme.typography.bodyLarge.copy(fontWeight = FontWeight.SemiBold))
            }
            if (draft.attachments.isNotEmpty() || model.attaching) {
                Row(Modifier.horizontalScroll(rememberScrollState()).padding(vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(6.dp),
                    verticalAlignment = Alignment.CenterVertically) {
                    draft.attachments.forEachIndexed { index, file ->
                        Row(Modifier.background(lagom.subtle, RoundedCornerShape(50)).padding(start = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                            LagomIcon(R.drawable.ic_attachment, size = 16.dp)
                            Text(file.filename, Modifier.padding(start = 4.dp).widthIn(max = 180.dp), maxLines = 1, overflow = TextOverflow.MiddleEllipsis,
                                style = MaterialTheme.typography.bodySmall)
                            LagomIconButton("Remove ${file.filename}", R.drawable.ic_close, enabled = !model.sending) { model.removeAttachment(index) }
                        }
                    }
                    if (model.attaching) CircularProgressIndicator(Modifier.size(20.dp), color = lagom.ink2, strokeWidth = 2.dp)
                }
                if (draft.attachmentsTooLarge) Text("Over 20 MB. Some mail servers refuse mail this large.", Modifier.padding(bottom = 6.dp), maxLines = 1,
                    overflow = TextOverflow.Ellipsis, color = lagom.ink2, style = MaterialTheme.typography.bodySmall)
            }
            Box(Modifier.fillMaxWidth().padding(vertical = 12.dp)) {
                val body = MaterialTheme.typography.bodyLarge.copy(fontSize = TextSize.body, lineHeight = TextSize.body * ReadingLeading)
                if (draft.body.isEmpty()) Text("Message · Markdown supported", Modifier.clearAndSetSemantics {}, color = lagom.ink2, style = body)
                BasicTextField(draft.body, { edit(draft.copy(body = it)) }, Modifier.fillMaxWidth().heightIn(min = 240.dp).semantics { contentDescription = "Message body" },
                    enabled = !model.sending, textStyle = body.copy(color = lagom.ink), cursorBrush = SolidColor(lagom.accent))
            }
            draft.quoted?.let { quoted ->
                var open by remember(draft.id) { mutableStateOf(model.demoQuoteOpen) }
                QuoteRow(open, enabled = !model.sending, toggle = { open = !open }) { open = false; edit(draft.copy(quoted = null)) }
                // The reading pane's web view and document: the same sanitized HTML and the same blocks on remote content.
                if (open) Box(Modifier.fillMaxWidth().height(280.dp).padding(bottom = 12.dp).clip(RoundedCornerShape(8.dp))
                    .border(1.dp, lagom.line, RoundedCornerShape(8.dp)).semantics { contentDescription = "Quoted message" }) {
                    MailHtml(messageId = 0, html = quoted.html)
                }
            }
        }
    }
}

/** The quiet "··· Quoted message" row under the editor: it opens the quote in place; × removes it from this message. */
@Composable private fun QuoteRow(open: Boolean, enabled: Boolean, toggle: () -> Unit, remove: () -> Unit) {
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        Row(Modifier.weight(1f).heightIn(min = 48.dp).clickable(role = Role.Button, onClickLabel = if (open) "Hide quoted message" else "Show quoted message", onClick = toggle),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("···", color = lagom.ink2, style = MaterialTheme.typography.bodySmall.copy(fontFamily = LabelFace))
            Text(if (open) "Hide quoted message" else "Quoted message", color = lagom.ink2, style = MaterialTheme.typography.bodySmall)
        }
        LagomIconButton("Remove quoted message", R.drawable.ic_close, enabled = enabled, onClick = remove)
    }
}

// shortcut: reads the whole file into memory with no cap, add a hard limit if people pick very large videos.
private fun ContentResolver.attachment(uri: Uri): SendAttachment {
    val name = query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { if (it.moveToFirst()) it.getString(0) else null }
    val data = requireNotNull(openInputStream(uri)) { "Could not open the file" }.use { it.readBytes() }
    return pickedAttachment(name, getType(uri), data)
}

@Composable private fun RecipientField(label: String, value: String, enabled: Boolean, core: LagomCoreInterface?, account: String,
                                      contactsReady: Boolean, change: (String) -> Unit) {
    var focused by remember { mutableStateOf(false) }
    val focus = remember { FocusRequester() }
    // The String overload keeps the old caret after a pick; own the selection so it moves to the end.
    var field by remember { mutableStateOf(TextFieldValue(value, TextRange(value.length))) }
    if (field.text != value) field = TextFieldValue(value, TextRange(value.length))
    // Suggestions follow the caret: editing an earlier recipient must not complete the last one.
    val atEnd = field.selection.collapsed && field.selection.end == field.text.length
    val query = if (atEnd) recipientToken(value) else ""
    Column {
        FieldRow(label) {
            BasicTextField(field, { field = it; if (it.text != value) change(it.text) }, enabled = enabled, singleLine = true,
                textStyle = MaterialTheme.typography.bodyLarge.copy(color = lagom.ink), cursorBrush = SolidColor(lagom.accent),
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Email, autoCorrectEnabled = false),
                modifier = Modifier.focusRequester(focus).onFocusChanged { focused = it.isFocused }.fillMaxWidth().semantics { contentDescription = label })
        }
        if (focused && enabled && contactsReady && query.isNotEmpty()) {
            core?.suggestContacts(account, query)?.forEach { contact ->
                Column(Modifier.fillMaxWidth().heightIn(min = 48.dp)
                    .semantics(mergeDescendants = true) { contentDescription = "${contact.name}, ${contact.address}" }
                    .clickable(role = Role.Button, onClickLabel = "Add recipient") {
                        val next = recipientToken(value, contact.recipient)
                        field = TextFieldValue(next, TextRange(next.length))
                        change(next)
                        focus.requestFocus()
                    }.padding(start = 80.dp, top = 6.dp, bottom = 6.dp), verticalArrangement = Arrangement.Center) {
                    if (contact.name.isNotEmpty()) Text(contact.name)
                    Text(contact.address, color = lagom.ink2, style = MaterialTheme.typography.bodySmall)
                }
            }
        }
    }
}

/** One labelled line, desktop compose `field`: a quiet label, the control, a hairline under. */
@Composable private fun FieldRow(label: String, modifier: Modifier = Modifier, content: @Composable RowScope.() -> Unit) {
    Row(modifier.fillMaxWidth().heightIn(min = 52.dp).drawHairline(lagom.line), verticalAlignment = Alignment.CenterVertically) {
        Text(label, Modifier.width(80.dp).clearAndSetSemantics {}, color = lagom.ink2, style = MaterialTheme.typography.bodySmall)
        content()
    }
}

@Composable private fun RowScope.LineInput(value: String, change: (String) -> Unit, enabled: Boolean, label: String, style: TextStyle) =
    BasicTextField(value, change, Modifier.weight(1f).semantics { contentDescription = label }, enabled = enabled, singleLine = true,
        textStyle = style.copy(color = lagom.ink), cursorBrush = SolidColor(lagom.accent))

internal fun server(host: String, port: String, username: String, tls: Boolean): MailServer {
    val number = port.toIntOrNull()
    require(number != null && number in 1..65535) { "Ports must be numbers from 1 to 65535." }
    return MailServer(host.trim(), number.toUShort(), username.trim(), if (tls) TransportSecurity.TLS else TransportSecurity.START_TLS)
}

internal class AccountForm {
    var email by mutableStateOf("")
    var password by mutableStateOf("")
    var passwordStep by mutableStateOf(false)
    var advanced by mutableStateOf(false)
    var imapHost by mutableStateOf("")
    var smtpHost by mutableStateOf("")
    var imapPort by mutableStateOf("993")
    var smtpPort by mutableStateOf("587")
    var imapUser by mutableStateOf("")
    var smtpUser by mutableStateOf("")
    var imapTls by mutableStateOf(true)
    var smtpTls by mutableStateOf(false)
    var connecting by mutableStateOf(false)
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable internal fun AddAccountScreen(model: MailModel, form: AccountForm, showProviders: Boolean = true, close: () -> Unit) {
    with(form) {
        BackHandler(model.accountForm === form) { if (!connecting) { password = ""; close() } }
        Scaffold(containerColor = lagom.bg, topBar = {
            TopBar(Modifier.statusBarsPadding(), start = 4.dp, end = 12.dp) {
                LagomIconButton("Cancel", R.drawable.ic_close, enabled = !connecting) { password = ""; close() }
                BarTitle("Add account", Modifier.weight(1f))
                HouseButton(if (passwordStep) "Sign in" else "Continue", primary = true,
                    enabled = !connecting && email.isNotBlank() && (!passwordStep || password.isNotEmpty()), onClick = {
                    if (!passwordStep) { passwordStep = true; imapUser = email; smtpUser = email }
                    else {
                        connecting = true
                        model.perform {
                            try {
                                val core = requireNotNull(model.core)
                                if (advanced) core.addAccountManual(email.trim(), password,
                                    server(imapHost, imapPort, imapUser, imapTls), server(smtpHost, smtpPort, smtpUser, smtpTls))
                                else core.addAccount(email.trim(), password)
                                password = ""
                                close()
                                model.reload()
                                model.sync()
                            } finally { connecting = false }
                        }
                    }
                })
            }
        }) { padding ->
            Column(Modifier.fillMaxSize().padding(padding).imePadding().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                // Hidden until this build has the provider's client ID.
                val context = LocalContext.current
                val providers = remember { listOf(SignInProvider.GOOGLE to "Google", SignInProvider.MICROSOFT to "Microsoft").filter { signInAvailable(it.first) } }
                if (showProviders) for ((provider, name) in providers) {
                    HouseButton("Sign in with $name", R.drawable.ic_connect, Modifier.fillMaxWidth(), enabled = !connecting) {
                        model.perform { openSignIn(context, requireNotNull(model.core).beginSignIn(provider).url) }
                    }
                }
                Field("Email", email, !connecting, KeyboardType.Email) { email = it }
                if (passwordStep) {
                    Field("Password", password, !connecting, KeyboardType.Password) { password = it }
                    Text(if (providers.any { it.first == SignInProvider.MICROSOFT }) "Gmail and iCloud need an app password, not your normal password. For Microsoft, use Sign in with Microsoft."
                        else "Gmail and iCloud need an app password, not your normal password. Some Microsoft accounts only work with Sign in with Microsoft, which this version does not have.", color = lagom.ink2, style = MaterialTheme.typography.bodySmall)
                    Row { Checkbox(advanced, { advanced = it }, enabled = !connecting); Text("Advanced", Modifier.padding(top = 12.dp)) }
                    if (advanced) {
                        Text("Incoming mail", style = MaterialTheme.typography.titleMedium)
                        Field("Incoming mail server", imapHost, !connecting, hint = "imap.example.com") { imapHost = it }
                        Field("Incoming mail port", imapPort, !connecting, KeyboardType.Number) { imapPort = it }
                        Field("Incoming mail username", imapUser, !connecting) { imapUser = it }
                        SecurityChoice(imapTls, !connecting) { imapTls = it }
                        Text("Outgoing mail", style = MaterialTheme.typography.titleMedium)
                        Field("Outgoing mail server", smtpHost, !connecting, hint = "smtp.example.com") { smtpHost = it }
                        Field("Outgoing mail port", smtpPort, !connecting, KeyboardType.Number) { smtpPort = it }
                        Field("Outgoing mail username", smtpUser, !connecting) { smtpUser = it }
                        SecurityChoice(smtpTls, !connecting) { smtpTls = it }
                    }
                }
                if (afterWait(connecting)) LinearProgressIndicator(Modifier.fillMaxWidth())
            }
        }
    }
}

@Composable private fun Field(label: String, value: String, enabled: Boolean = true, keyboard: KeyboardType = KeyboardType.Text, hint: String? = null, change: (String) -> Unit) {
    OutlinedTextField(value, change, enabled = enabled, label = { Text(label) }, placeholder = hint?.let { { Text(it) } }, singleLine = true,
        keyboardOptions = KeyboardOptions(keyboardType = keyboard, autoCorrectEnabled = false),
        visualTransformation = if (keyboard == KeyboardType.Password) PasswordVisualTransformation() else VisualTransformation.None,
        modifier = Modifier.fillMaxWidth())
}
@Composable private fun SecurityChoice(tls: Boolean, enabled: Boolean, change: (Boolean) -> Unit) {
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        FilterChip(tls, { change(true) }, label = { Text("TLS") }, enabled = enabled)
        FilterChip(!tls, { change(false) }, label = { Text("STARTTLS") }, enabled = enabled)
    }
}

/** The desktop's per-account signature: added to new mail, replies and forwards. */
@Composable internal fun SignatureDialog(model: MailModel, account: MailAccount, close: () -> Unit) {
    // Saveable, so an edit not yet saved survives rotation; only a fresh dialog loads the stored signature.
    var text by rememberSaveable(account.id) { mutableStateOf<String?>(null) }
    var saving by remember(account.id) { mutableStateOf(false) }
    LaunchedEffect(account.id) {
        if (text != null) return@LaunchedEffect
        try { text = requireNotNull(model.core).signature(account.id) }
        catch (cancelled: CancellationException) { throw cancelled }
        catch (e: Exception) { model.error = e.mailMessage(); close() }
    }
    AlertDialog(onDismissRequest = { if (!saving) close() }, title = { Text("Signature") }, text = {
        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(account.email, style = MaterialTheme.typography.bodySmall)
            OutlinedTextField(text.orEmpty(), { text = it }, enabled = text != null && !saving, minLines = 4,
                label = { Text("Added to new mail, replies and forwards") }, modifier = Modifier.fillMaxWidth())
        }
    }, confirmButton = {
        TextButton(enabled = text != null && !saving, modifier = Modifier.heightIn(min = 48.dp), onClick = {
            saving = true
            model.perform {
                try { requireNotNull(model.core).setSignature(account.id, text.orEmpty()); close() }
                finally { saving = false }
            }
        }) { Text("Save") }
    }, dismissButton = {
        TextButton(enabled = !saving, modifier = Modifier.heightIn(min = 48.dp), onClick = close) { Text("Cancel") }
    })
}

/** Desktop's "Your name": shown to the people you write to. An empty name falls back to the address. */
@Composable internal fun NameDialog(model: MailModel, account: MailAccount, close: () -> Unit) {
    var text by rememberSaveable(account.id) { mutableStateOf(if (account.displayName == account.email) "" else account.displayName) }
    var saving by remember(account.id) { mutableStateOf(false) }
    AlertDialog(onDismissRequest = { if (!saving) close() }, title = { Text("Your name") }, text = {
        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(account.email, style = MaterialTheme.typography.bodySmall)
            OutlinedTextField(text, { text = it }, enabled = !saving, singleLine = true,
                label = { Text("Shown to the people you write to") }, modifier = Modifier.fillMaxWidth())
        }
    }, confirmButton = {
        TextButton(enabled = !saving, modifier = Modifier.heightIn(min = 48.dp), onClick = {
            saving = true
            model.perform {
                try { model.setName(account, text); close() }
                finally { saving = false }
            }
        }) { Text("Save") }
    }, dismissButton = {
        TextButton(enabled = !saving, modifier = Modifier.heightIn(min = 48.dp), onClick = close) { Text("Cancel") }
    })
}

/** Desktop's "New folder": a name, then Create. A refusal shows core's words and keeps the name. */
@Composable internal fun NewFolderDialog(model: MailModel, request: NewFolder) {
    // The name survives a rotation; the busy state lives in the model with the call it guards.
    var name by rememberSaveable(request) { mutableStateOf("") }
    val saving = model.creatingFolder
    val close = { if (model.newFolder == request) model.newFolder = null }
    AlertDialog(onDismissRequest = { if (!saving) close() }, title = { Text("New folder") }, text = {
        OutlinedTextField(name, { name = it }, enabled = !saving, singleLine = true,
            label = { Text("Folder name") }, modifier = Modifier.fillMaxWidth())
    }, confirmButton = {
        TextButton(enabled = !model.working, modifier = Modifier.heightIn(min = 48.dp), onClick = {
            model.perform { model.createFolder(request, name) }
        }) { Text("Create") }
    }, dismissButton = {
        TextButton(enabled = !saving, modifier = Modifier.heightIn(min = 48.dp), onClick = close) { Text("Cancel") }
    })
}
