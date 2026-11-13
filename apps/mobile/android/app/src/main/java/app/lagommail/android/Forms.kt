package app.lagommail.android

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp
import app.lagommail.core.*

@OptIn(ExperimentalMaterial3Api::class)
@Composable internal fun ComposeScreen(model: MailModel) {
    val draft = model.draft ?: return
    var cancelling by remember(draft.id) { mutableStateOf(false) }
    BackHandler { if (!model.sending) cancelling = true }
    if (cancelling) {
        AlertDialog(onDismissRequest = { cancelling = false }, title = { Text("Save draft?") },
            confirmButton = { TextButton(enabled = !model.sending, modifier = Modifier.heightIn(min = 48.dp), onClick = {
                cancelling = false; model.perform { model.closeCompose(discard = false) }
            }) { Text("Keep draft") } },
            dismissButton = { TextButton(enabled = !model.sending, modifier = Modifier.heightIn(min = 48.dp), onClick = {
                cancelling = false; model.perform { model.closeCompose(discard = true) }
            }) { Text("Discard") } })
    }
    Scaffold(topBar = {
        TopAppBar(title = { Text(if (draft.isReply) "Reply" else if (draft.restored) "Draft" else "New message") },
            navigationIcon = { TextButton(enabled = !model.sending, modifier = Modifier.heightIn(min = 48.dp), onClick = { cancelling = true }) { Text("Cancel") } },
            actions = { TextButton(enabled = !model.sending && (draft.to.isNotBlank() || draft.cc.isNotBlank()) && draft.account.isNotBlank(),
                modifier = Modifier.heightIn(min = 48.dp), onClick = { model.perform { model.send() } }) { Text(if (model.sending) "Queuing…" else "Send") } })
    }) { padding ->
        Column(Modifier.fillMaxSize().padding(padding).imePadding().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            var fromMenu by remember { mutableStateOf(false) }
            Box {
                OutlinedButton(enabled = !model.sending && !draft.isReply && !draft.restored, onClick = { fromMenu = true }) {
                    Text("From: ${model.accounts.find { it.id == draft.account }?.email.orEmpty()}")
                }
                DropdownMenu(fromMenu, { fromMenu = false }) {
                    model.accounts.forEach { account ->
                        DropdownMenuItem(text = { Text(account.email) }, onClick = { fromMenu = false; model.perform { model.changeComposeAccount(account.id) } })
                    }
                }
            }
            Field("To", draft.to, !model.sending, KeyboardType.Email) { model.draft = draft.copy(to = it) }
            Field("Cc", draft.cc, !model.sending, KeyboardType.Email) { model.draft = draft.copy(cc = it) }
            Field("Subject", draft.subject, !model.sending) { model.draft = draft.copy(subject = it) }
            if (draft.attachments.isNotEmpty()) Text("Attachments: ${draft.attachments.joinToString { it.filename }}")
            OutlinedTextField(draft.body, { model.draft = draft.copy(body = it) }, enabled = !model.sending,
                label = { Text("Message · Markdown supported") }, minLines = 12, modifier = Modifier.fillMaxWidth())
        }
    }
}

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
@Composable internal fun AddAccountScreen(model: MailModel, close: () -> Unit) {
    val form = model.accountForm ?: return
    with(form) {
        BackHandler { if (!connecting) { password = ""; close() } }
        Scaffold(topBar = {
            TopAppBar(title = { Text("Add account") }, navigationIcon = {
                TextButton(enabled = !connecting, onClick = { password = ""; close() }) { Text("Cancel") }
            }, actions = {
                TextButton(enabled = !connecting && email.isNotBlank() && (!passwordStep || password.isNotEmpty()), onClick = {
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
                }) { Text(if (passwordStep) "Connect" else "Continue") }
            })
        }) { padding ->
            Column(Modifier.fillMaxSize().padding(padding).imePadding().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Field("Email", email, !connecting, KeyboardType.Email) { email = it }
                if (passwordStep) {
                    Field("Password", password, !connecting, KeyboardType.Password) { password = it }
                    // shortcut: app passwords only, add OAuth when mobile client IDs are provisioned.
                    Text("Gmail and iCloud need an app-specific password. Microsoft may require OAuth, which is not available in this version.", style = MaterialTheme.typography.bodySmall)
                    Row { Checkbox(advanced, { advanced = it }, enabled = !connecting); Text("Advanced", Modifier.padding(top = 12.dp)) }
                    if (advanced) {
                        Text("IMAP", style = MaterialTheme.typography.titleMedium)
                        Field("IMAP host", imapHost, !connecting) { imapHost = it }
                        Field("IMAP port", imapPort, !connecting, KeyboardType.Number) { imapPort = it }
                        Field("IMAP username", imapUser, !connecting) { imapUser = it }
                        SecurityChoice(imapTls, !connecting) { imapTls = it }
                        Text("SMTP", style = MaterialTheme.typography.titleMedium)
                        Field("SMTP host", smtpHost, !connecting) { smtpHost = it }
                        Field("SMTP port", smtpPort, !connecting, KeyboardType.Number) { smtpPort = it }
                        Field("SMTP username", smtpUser, !connecting) { smtpUser = it }
                        SecurityChoice(smtpTls, !connecting) { smtpTls = it }
                    }
                }
                if (connecting) LinearProgressIndicator(Modifier.fillMaxWidth())
            }
        }
    }
}

@Composable private fun Field(label: String, value: String, enabled: Boolean = true, keyboard: KeyboardType = KeyboardType.Text, change: (String) -> Unit) {
    OutlinedTextField(value, change, enabled = enabled, label = { Text(label) }, singleLine = true,
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
