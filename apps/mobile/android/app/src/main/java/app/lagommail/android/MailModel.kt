package app.lagommail.android

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import app.lagommail.core.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import java.time.Instant
import java.util.UUID

internal data class RowPresentation(val row: MailRow) {
    val sender = row.senderName.ifEmpty { row.senderAddress }
    val subject = row.subject.ifEmpty { "(No subject)" }
    val date: Instant = Instant.ofEpochSecond(row.dateUnix)
}
internal fun draftMessageId(email: String): String =
    "<${UUID.randomUUID().toString().replace("-", "")}@${email.substringAfterLast('@', "localhost")}>"

internal fun safeAttachmentName(filename: String): String {
    val name = filename.replace('\\', '/').substringAfterLast('/').filterNot { it.isISOControl() }
        .trim().take(120).trim()
    return name.takeUnless { it.isEmpty() || it == "." || it == ".." } ?: "attachment"
}

/** Total size above which compose shows a one-line warning; many servers refuse mail near 25 MB. */
internal const val ATTACHMENT_WARNING_BYTES = 20 * 1024 * 1024

internal fun pickedAttachment(name: String?, mimeType: String?, data: ByteArray) =
    SendAttachment(safeAttachmentName(name.orEmpty()), mimeType ?: "application/octet-stream", data)

// Same boundaries as core split_recipients; desktop keeps the unfinished token in its input.
internal fun recipientToken(value: String, recipient: String? = null): String {
    var quoted = false; var escaped = false; var angle = false
    var comments = 0
    var start = 0
    value.forEachIndexed { at, c ->
        if (escaped) { escaped = false; return@forEachIndexed }
        when {
            c == '\\' && (quoted || comments > 0) -> escaped = true
            c == '"' && comments == 0 -> quoted = !quoted
            c == '(' && !quoted -> comments++
            c == ')' && !quoted -> comments = (comments - 1).coerceAtLeast(0)
            c == '<' && !quoted && comments == 0 -> angle = true
            c == '>' && !quoted && comments == 0 -> angle = false
            c in ",;\n\r" && !quoted && !angle && comments == 0 -> start = at + 1
        }
    }
    while (start < value.length && value[start].isWhitespace()) start++
    return if (recipient != null) value.substring(0, start) + recipient + ", " else value.substring(start).trim()
}

internal data class ComposeDraft(
    val account: String, val to: String = "", val cc: String = "", val subject: String = "",
    val body: String = "", val replyTo: Long? = null,
    val id: String = draftMessageId(""),
    val attachments: List<SendAttachment> = emptyList(), val restored: Boolean = false,
    val isReply: Boolean = replyTo != null,
    val title: String = if (replyTo == null) "New message" else "Reply",
    /** Only a mailto link or a saved draft fills Bcc, and then the field shows, as on desktop. */
    val bcc: String = "", val showBcc: Boolean = bcc.isNotEmpty(),
    /** The body a new message starts with, the signature: unchanged, it is not worth a draft. */
    val startBody: String = "",
) {
    constructor(reply: ReplyDraft, kind: ComposeKind = ComposeKind.REPLY, email: String = "") : this(
        reply.account, reply.to, reply.cc, reply.subject, reply.body, reply.replyTo, id = draftMessageId(email), attachments = reply.attachments,
        title = when (kind) { ComposeKind.REPLY -> "Reply"; ComposeKind.REPLY_ALL -> "Reply all"; ComposeKind.FORWARD -> "Forward" })
    constructor(saved: LocalDraft) : this(saved.account, saved.to, saved.cc, saved.subject, saved.body,
        id = saved.id, attachments = saved.attachments, restored = true, isReply = saved.inReplyTo != null || saved.references.isNotEmpty(),
        title = if (saved.inReplyTo != null || saved.references.isNotEmpty()) "Reply" else "Draft", bcc = saved.bcc)
    val hasContent get() = to.isNotEmpty() || cc.isNotEmpty() || bcc.isNotEmpty() || subject.isNotEmpty() || body != startBody || attachments.isNotEmpty()
    val attachmentsTooLarge get() = attachments.sumOf { it.data.size.toLong() } > ATTACHMENT_WARNING_BYTES
}
/** A mailto link or a share from another app, with the shared files. */
internal data class SharedMail(val mail: Mailto, val attachments: List<SendAttachment> = emptyList())

private fun headerText(value: String) = value.map { if (it.isISOControl()) ' ' else it }.joinToString("").trim()

/** `parse` is the core mailto parser; extras fill what the link leaves empty. `null`: not mail to open. */
internal fun sharedMail(action: String?, data: String?, email: List<String>, cc: List<String>, bcc: List<String>,
    subject: String?, text: String?, parse: (String) -> Mailto?): Mailto? {
    val link = when (action) {
        "android.intent.action.SENDTO", "android.intent.action.VIEW" -> parse(data ?: return null) ?: return null
        "android.intent.action.SEND", "android.intent.action.SEND_MULTIPLE" -> Mailto(emptyList(), emptyList(), emptyList(), "", "")
        else -> return null
    }
    // Extras come from any app, so they get the same CRLF rule as mailto headers.
    fun addresses(values: List<String>) = values.map(::headerText).filter { it.isNotEmpty() }
    return Mailto(link.to + addresses(email), link.cc + addresses(cc), link.bcc + addresses(bcc),
        link.subject.ifEmpty { headerText(subject.orEmpty()) },
        link.body.ifEmpty { text.orEmpty().replace("\r\n", "\n").replace('\r', '\n') })
}

/** The link's body sits above the signature. Its Bcc shows in its own field, so a link never adds a hidden recipient unseen. */
internal fun sharedDraft(account: String, email: String, shared: SharedMail, signature: String = "") = ComposeDraft(account,
    shared.mail.to.joinToString(", "), shared.mail.cc.joinToString(", "), shared.mail.subject, shared.mail.body + signature,
    id = draftMessageId(email), attachments = shared.attachments, bcc = shared.mail.bcc.joinToString(", "))

internal enum class MailAction(val notice: String) { Done("Moved to Done"), Later("Moved to Later"), Delete("Deleted") }

internal class MailModel(
    private val open: suspend () -> LagomCoreInterface,
    private val attachmentDirectory: File,
    private val now: () -> Long = { System.currentTimeMillis() },
    /** Sends the outbox from WorkManager, which outlives a frozen or killed process. */
    private val sendLater: () -> Unit = {},
) : ViewModel() {
    var downloadingAttachments by mutableStateOf<Set<Pair<Long, String>>>(emptySet()); private set
    var core: LagomCoreInterface? = null; private set
    var accounts by mutableStateOf<List<MailAccount>>(emptyList()); private set
    var rows by mutableStateOf<List<MailRow>>(emptyList()); private set
    var failures by mutableStateOf<List<SendFailure>>(emptyList()); private set
    var accountId by mutableStateOf<String?>(null); private set
    var mailbox by mutableStateOf(Mailbox.INBOX); private set
    var searchText by mutableStateOf(""); private set
    var unreadOnly by mutableStateOf(false)
    val visibleRows: List<MailRow> get() = if (unreadOnly) rows.filter { it.unread } else rows
    var selectedId by mutableStateOf<Long?>(null); private set
    var message by mutableStateOf<MailMessage?>(null); private set
    var messages by mutableStateOf<List<MailMessage>>(emptyList()); private set
    var expandedIds by mutableStateOf<Set<Long>>(emptySet()); private set
    var remoteImagesIds by mutableStateOf<Set<Long>>(emptySet()); private set
    fun loadRemoteImages(id: Long) { if (messages.any { it.row.id == id }) remoteImagesIds += id }
    var hasMore by mutableStateOf(false); private set
    var syncing by mutableStateOf(false); private set
    private var actingId by mutableStateOf<Long?>(null)
    private var readingIds by mutableStateOf<Set<Long>>(emptySet())
    val working: Boolean get() = actingId != null || readingIds.isNotEmpty()
    var sending by mutableStateOf(false); private set
    var loading by mutableStateOf(true); private set
    var error by mutableStateOf<String?>(null)
    var notice by mutableStateOf<String?>(null)
    var undoSendId by mutableStateOf<String?>(null); private set
    private var sendRevision = 0
    /** What the last Done, Later or Delete changed, while its notice offers Undo. */
    var undoChanges by mutableStateOf<List<MailChange>>(emptyList()); private set
    var drafts by mutableStateOf<List<DraftRow>>(emptyList()); private set
    var showingDrafts by mutableStateOf(false); private set
    var savedDraftIds by mutableStateOf<Set<String>>(emptySet()); private set
    private var draftRevision = 0
    private val draftLock = Mutex()
    private var autosave: Job? = null
    private var endingCompose = false
    private val finishedDrafts = mutableSetOf<String>()
    private var composeRevision = 0
    private var composeDraft by mutableStateOf<ComposeDraft?>(null)
    var draft: ComposeDraft?
        get() = composeDraft
        set(value) {
            val old = composeDraft
            composeRevision++; composeDraft = value
            if (endingCompose || old == value) return
            autosave?.cancel()
            if (value != null && (value.hasContent || old?.hasContent == true || value.id in savedDraftIds)) {
                autosave = perform { delay(500); persist(value) }
            } else if (value == null && old != null) {
                perform { persist(old) }
            }
        }
    var attaching by mutableStateOf(false); private set
    /** Reads picked files off the main thread and adds them to the draft they were picked for. */
    suspend fun attach(read: () -> List<SendAttachment>) {
        val id = draft?.id ?: return
        attaching = true
        try {
            val files = withContext(Dispatchers.IO) { read() }
            draft?.takeIf { it.id == id }?.let { draft = it.copy(attachments = it.attachments + files) }
        } finally { attaching = false }
    }
    fun removeAttachment(index: Int) { draft?.let { draft = it.copy(attachments = it.attachments.filterIndexed { at, _ -> at != index }) } }
    var accountForm by mutableStateOf<AccountForm?>(null)
    private var revision = 0
    private var appliedRevision = 0
    private var selectionRevision = 0
    private var syncAgain = false
    private var foreground = false
    private var wake: Job? = null

    /** The browser's redirect after "Sign in with Google" or Microsoft. The core refuses all but the pending sign-in. */
    fun finishSignIn(redirect: String) = perform {
        val form = accountForm
        form?.connecting = true
        try {
            requireNotNull(core).finishSignIn(redirect)
            if (accountForm === form) accountForm = null
            reload()
            sync()
        } finally { form?.connecting = false }
    }
    fun perform(action: suspend () -> Unit) = viewModelScope.launch {
        try { action() } catch (e: CancellationException) { throw e }
        catch (e: Exception) { error = e.mailMessage() }
    }
    suspend fun start() {
        if (core != null) return
        try {
            // Best effort: a stale cache must never block opening mail.
            withContext(Dispatchers.IO) { attachmentDirectory.deleteRecursively() }
            core = open(); reload()
        } finally { loading = false }
    }
    suspend fun openAttachment(messageId: Long, part: MailAttachment): File? {
        val core = core ?: return null
        val key = messageId to part.section
        if (key in downloadingAttachments) return null
        downloadingAttachments += key
        val selection = selectionRevision
        try {
            val data = core.attachment(messageId, part.section)
            val file = withContext(Dispatchers.IO) {
                val directory = File(attachmentDirectory, UUID.randomUUID().toString())
                check(directory.mkdirs()) { "Could not create attachment cache" }
                File(directory, safeAttachmentName(part.filename)).also { it.writeBytes(data) }
            }
            return file.takeIf { selection == selectionRevision }
        } finally { downloadingAttachments -= key }
    }
    suspend fun setForeground(active: Boolean) {
        foreground = active
        wake?.cancel(); wake = null
        if (active) { start(); reload(); sync() }
        else if (!sending) draft?.let { persist(it) }
        // Background refresh never sends, so mail inside the Undo window needs its own job.
        if (!active && undoSendId != null) sendLater()
    }
    suspend fun reload(more: Boolean = false) {
        val core = core ?: return
        // Pending reloads may have changed the rows before the pagination offset.
        val append = more && revision == appliedRevision
        val request = ++revision
        try {
            val old = rows
            val offset = if (append) old.size else 0
            val account = accountId
            val box = mailbox
            val query = searchText.trim()
            val due = core.nextSnooze()
            val accounts = core.accounts()
            val page = mutableListOf<MailRow>()
            val count = maxOf(100, old.size + if (more) 100 else 0) - offset
            var hasMore: Boolean
            do {
                val batch = if (query.isEmpty()) core.mailbox(account, box, 100u, (offset + page.size).toUInt())
                    else core.search(account, query, 100u, (offset + page.size).toUInt())
                page += batch
                hasMore = batch.size == 100
            } while (hasMore && page.size < count)
            val failures = core.sendFailures()
            if (request != revision) return
            this.accounts = accounts
            this.failures = failures
            rows = (if (append) old + page else page).distinctBy { it.id }
            this.hasMore = hasMore
            appliedRevision = request
            openPendingShare()
            wake?.cancel()
            wake = if (foreground && due != null) viewModelScope.launch {
                delay(maxOf(1, due * 1000 - now()))
                wake = null
                try { reload() } catch (e: CancellationException) { throw e }
                catch (e: Exception) { error = e.mailMessage() }
            } else null
        } catch (e: CancellationException) { throw e }
        catch (e: Exception) { if (request == revision) throw e }
        val draftRequest = draftRevision
        try {
            val drafts = core.localDrafts()
            if (request != revision || draftRequest != draftRevision) return
            this.drafts = drafts
            savedDraftIds = savedDraftIds + drafts.map { it.id }
        } catch (e: CancellationException) { throw e }
        catch (e: Exception) { if (request == revision) error = e.mailMessage() }
    }
    /** Waits for the store and the accounts; `reload` opens it. */
    private var pendingShare: SharedMail? = null
    suspend fun openShared(shared: SharedMail) { pendingShare = shared; reload() }
    private suspend fun openPendingShare() {
        val shared = pendingShare ?: return
        pendingShare = null
        val account = accountId ?: accounts.firstOrNull()?.id ?: run { error = "Add an account, then open the link again."; return }
        closeCompose(discard = false)
        if (draft != null) { error = "Finish the open message first."; return }
        accountForm = null
        val signature = requireNotNull(core).newMailBody(account)
        if (draft != null) return
        draft = sharedDraft(account, accounts.firstOrNull { it.id == account }?.email.orEmpty(), shared, signature)
    }
    /** A new message from the account, starting with its signature. */
    suspend fun newMessage(account: String) {
        val core = core ?: return
        if (draft != null) return
        val request = ++composeRevision
        val body = core.newMailBody(account)
        if (request != composeRevision || draft != null) return
        draft = ComposeDraft(account, body = body, startBody = body, id = draftMessageId(accounts.firstOrNull { it.id == account }?.email.orEmpty()))
    }
    suspend fun openNotification(target: MailTarget) {
        closeCompose(discard = false)
        accountForm = null
        searchText = ""
        enterScope(target.account, Mailbox.INBOX)
        // Every navigation bumps selectionRevision; never pull the user back after one.
        val selection = selectionRevision
        reload()
        if (selection == selectionRevision) select(target.message)
    }
    suspend fun changeScope(account: String?, box: Mailbox) {
        enterScope(account, box)
        reload()
    }
    private fun enterScope(account: String?, box: Mailbox) {
        showingDrafts = false
        revision++
        clearSelection()
        accountId = account; mailbox = box; rows = emptyList(); hasMore = false
    }
    suspend fun changeSearch(text: String) {
        if (searchText == text) return
        revision++
        clearSelection()
        searchText = text; rows = emptyList(); hasMore = false
        reload()
    }
    fun clearSelection() { selectionRevision++; selectedId = null; message = null; messages = emptyList(); expandedIds = emptySet(); remoteImagesIds = emptySet() }
    suspend fun select(id: Long) {
        val core = core ?: return
        clearSelection()
        val request = selectionRevision
        selectedId = id
        try {
            val loaded = withContext(Dispatchers.IO) { core.conversation(id) }
            if (request != selectionRevision) return
            messages = loaded
            message = loaded.firstOrNull { it.row.id == id } ?: loaded.lastOrNull()
            expandedIds = loaded.filter { it.row.id == id || it.row.unread || it.row.id == loaded.lastOrNull()?.row?.id }.map { it.row.id }.toSet()
            openBodies(loaded.filter { it.row.id in expandedIds }.map { it.row.id }, request) {
                for (item in loaded.filter { it.row.unread }) {
                    if (request != selectionRevision) return@openBodies
                    setRead(item.row.id, true)
                }
            }
        } catch (e: CancellationException) { throw e }
        catch (e: Exception) { if (request == selectionRevision) throw e }
    }
    suspend fun toggleMessage(id: Long) {
        val item = messages.firstOrNull { it.row.id == id } ?: return
        if (id in expandedIds) { expandedIds -= id; return }
        expandedIds += id
        openBodies(listOf(id), selectionRevision) { if (item.row.unread) setRead(id, true) }
    }
    /** Sends the read flags, then downloads missing bodies; a refused flag stops no download.
     *  The core runs one network call at a time, so flags first keeps a slow body from holding them back. */
    private suspend fun openBodies(ids: List<Long>, request: Int, read: suspend () -> Unit) {
        val refused = try { read(); null } catch (e: CancellationException) { throw e } catch (e: Exception) { e }
        for (id in ids) { if (request != selectionRevision) break; loadBody(id, request) }
        refused?.let { throw it }
    }
    /** Sync leaves some bodies out (over 4 MiB, or synced before HTML parts were kept): for mail
     *  without HTML, load_inline fetches the body, at most once per message. */
    private suspend fun loadBody(id: Long, request: Int) {
        val core = core ?: return
        val item = messages.firstOrNull { it.row.id == id }?.takeIf { it.htmlBody == null } ?: return
        try {
            val updated = withContext(Dispatchers.IO) { core.loadInline(id); core.message(id) }
            if (request != selectionRevision || (updated.htmlBody == null && updated.textBody == item.textBody)) return
            messages = messages.map { if (it.row.id == id) updated else it }
            if (message?.row?.id == id) message = updated
            // The list snippet changed with the body.
            reload()
        } catch (e: CancellationException) { throw e }
        catch (e: Exception) { if (request == selectionRevision) error = "Could not download this message: ${e.mailMessage()}" }
    }
    suspend fun setRead(id: Long, read: Boolean) {
        val core = core ?: return
        if (actingId == id || id in readingIds) return
        readingIds += id
        try {
            core.markRead(id, read)
            if (messages.any { it.row.id == id }) {
                val selection = selectionRevision
                val updated = withContext(Dispatchers.IO) { core.message(id) }
                if (selection == selectionRevision) {
                    messages = messages.map { if (it.row.id == id) updated else it }
                    if (message?.row?.id == id) message = updated
                }
            }
        } finally { readingIds -= id; reload() }
    }
    suspend fun sync() {
        val core = core ?: return
        if (syncing) { syncAgain = true; return }
        syncing = true
        try {
            do {
                syncAgain = false
                try { core.syncAll() } catch (e: CancellationException) { throw e }
                catch (e: Exception) { error = e.mailMessage() }
                reload()
            } while (syncAgain)
        } finally { syncing = false }
    }
    suspend fun act(action: MailAction, id: Long, conversation: Boolean = true, until: Long? = null) {
        val core = core ?: return
        if (working) return
        val ids = if (conversation) rows.firstOrNull { id in it.messageIds }?.messageIds ?: listOf(id) else listOf(id)
        actingId = id
        try {
            val changes = when (action) {
                MailAction.Done -> core.done(ids)
                MailAction.Later -> core.later(ids, until ?: core.laterChoices()[1].untilUnix)
                MailAction.Delete -> core.delete(ids)
            }
            if (selectedId == id || messages.any { it.row.id in ids }) clearSelection()
            offerUndo(changes, action.notice)
        // Busy until the list shows the result, so a swiped row never comes back before it leaves.
        } finally { try { reload() } finally { actingId = null } }
    }
    /** The newest action owns the notice, so its Undo replaces an older one, also Undo send. */
    private fun offerUndo(changes: List<MailChange>, text: String) {
        if (changes.isEmpty()) return
        sendRevision++
        undoSendId = null
        undoChanges = changes
        notice = text
        // The same 5 s window as Undo send.
        perform {
            delay(5000)
            if (undoChanges === changes) { undoChanges = emptyList(); notice = null }
        }
    }
    /** Undo for the last Done, Later or Delete: each message goes back where it was. */
    suspend fun undoAction() {
        val core = core ?: return
        val changes = undoChanges
        if (changes.isEmpty() || working) return
        undoChanges = emptyList()
        notice = null
        actingId = changes.first().id
        try { core.undo(changes) } finally { try { reload() } finally { actingId = null } }
    }
    /** The reading view's "Unsubscribe from …" line: the run in progress, and what finished this session. */
    var unsubscribingId by mutableStateOf<Long?>(null); private set
    var unsubscribedIds by mutableStateOf<Set<Long>>(emptySet()); private set
    /** Only after the person confirmed. A web page is the screen's to open; the core never fetches it. */
    suspend fun unsubscribe(id: Long) {
        val core = core ?: return
        if (unsubscribingId != null) return
        unsubscribingId = id
        try {
            core.unsubscribe(id)
            unsubscribedIds += id
        } finally { unsubscribingId = null }
    }
    suspend fun remove(account: MailAccount) {
        core?.removeAccount(account.id)
        changeScope(if (accountId == account.id) null else accountId, mailbox)
    }
    suspend fun reply(id: Long, kind: ComposeKind = ComposeKind.REPLY) {
        val core = core ?: return
        if (draft != null) return
        val request = ++composeRevision
        val selection = selectionRevision
        // Like desktop R: only a plain reply reuses its saved draft.
        val saved = if (kind == ComposeKind.REPLY) core.savedReply(id) else null
        val prepared = if (saved != null) ComposeDraft(saved) else {
            val reply = core.composeDraft(id, kind)
            ComposeDraft(reply, kind, accounts.firstOrNull { it.id == reply.account }?.email.orEmpty())
        }
        if (request == composeRevision && selection == selectionRevision) {
            if (saved != null) { savedDraftIds = savedDraftIds + saved.id; finishedDrafts.remove(saved.id) }
            draft = prepared
        }
    }
    suspend fun showDrafts() {
        clearSelection()
        showingDrafts = true
        reload()
    }
    suspend fun openDraft(row: DraftRow) = draftLock.withLock {
        val core = core ?: return@withLock
        if (draft != null) return@withLock
        val saved = try { core.localDraft(row.id) } catch (e: MailException.NotFound) {
            draftRevision++
            drafts = drafts.filterNot { it.id == row.id }
            savedDraftIds = savedDraftIds - row.id
            return@withLock
        }
        if (draft != null) return@withLock
        savedDraftIds = savedDraftIds + saved.id
        finishedDrafts.remove(saved.id)
        draft = ComposeDraft(saved)
    }
    private suspend fun persist(snapshot: ComposeDraft) = draftLock.withLock {
        val core = core ?: return@withLock
        if (snapshot.id in finishedDrafts || (!snapshot.hasContent && snapshot.id !in savedDraftIds)) return@withLock
        // Rust may finish a write after coroutine cancellation; keep the lock until it completes.
        withContext(NonCancellable) {
            try {
                val saved = core.saveDraft(snapshot.id, snapshot.account, snapshot.to, snapshot.cc, snapshot.bcc, snapshot.subject,
                    snapshot.body, snapshot.replyTo, snapshot.attachments)
                draftRevision++
                savedDraftIds = savedDraftIds + saved.id
                drafts = listOf(saved) + drafts.filterNot { it.id == saved.id }
            } catch (e: Exception) {
                if (draft == null && snapshot.id !in finishedDrafts) {
                    endingCompose = true
                    draft = snapshot
                    endingCompose = false
                }
                throw e
            }
        }
    }
    suspend fun changeComposeAccount(account: String) {
        val core = core ?: return
        val old = draft ?: return
        if (sending || old.isReply || old.restored || old.account == account) return
        sending = true
        try {
            autosave?.cancelAndJoin()
            draftLock.withLock {
                withContext(NonCancellable) {
                    var replacement = old.copy(id = draftMessageId(accounts.firstOrNull { it.id == account }?.email.orEmpty()), account = account)
                    // An untouched body is the old account's signature; the new account brings its own.
                    if (old.body == old.startBody) core.newMailBody(account).let { replacement = replacement.copy(body = it, startBody = it) }
                    if (old.hasContent || old.id in savedDraftIds) {
                        // Save the replacement first so a failed account change cannot lose the original.
                        val saved = core.saveDraft(replacement.id, account, old.to, old.cc, old.bcc, old.subject, replacement.body, null, old.attachments)
                        draftRevision++
                        drafts = listOf(saved) + drafts
                        savedDraftIds = savedDraftIds + saved.id
                        core.deleteDraft(old.id)
                    }
                    draftRevision++
                    drafts = drafts.filterNot { it.id == old.id }
                    savedDraftIds = savedDraftIds - old.id
                    finishedDrafts += old.id
                    endingCompose = true
                    draft = replacement
                    endingCompose = false
                }
            }
        } finally { sending = false }
    }
    suspend fun closeCompose(discard: Boolean) {
        val core = core ?: return
        val snapshot = draft ?: return
        if (sending) return
        sending = true
        try {
            autosave?.cancelAndJoin()
            if (!discard) persist(snapshot)
            draftLock.withLock {
                withContext(NonCancellable) {
                    if (discard) {
                        core.deleteDraft(snapshot.id)
                        draftRevision++
                        drafts = drafts.filterNot { it.id == snapshot.id }
                        savedDraftIds = savedDraftIds - snapshot.id
                    }
                    finishCompose(snapshot.id)
                }
            }
        } finally { sending = false }
    }
    private fun finishCompose(id: String) {
        finishedDrafts += id
        endingCompose = true
        draft = null
        endingCompose = false
    }
    suspend fun send() {
        val core = core ?: return
        val snapshot = draft ?: return
        if (sending) return
        sending = true
        try {
            autosave?.cancelAndJoin()
            persist(snapshot)
            draftLock.withLock {
                withContext(NonCancellable) {
                    core.sendDraft(snapshot.id)
                    draftRevision++
                    drafts = drafts.filterNot { it.id == snapshot.id }
                    savedDraftIds = savedDraftIds - snapshot.id
                    finishCompose(snapshot.id)
                    val request = ++sendRevision
                    undoChanges = emptyList()
                    undoSendId = snapshot.id
                    notice = "Sending…"
                    perform {
                        delay(5000)
                        if (sendRevision == request && undoSendId == snapshot.id) { undoSendId = null; notice = null }
                        if (foreground) sync() else sendLater()
                    }
                }
            }
        } finally { sending = false }
    }
    suspend fun undoSend() {
        val core = core ?: return
        val id = undoSendId ?: return
        if (sending || draft != null) return
        sending = true
        try {
            draftLock.withLock {
                withContext(NonCancellable) {
                    if (core.undoSend(id)) {
                        draftRevision++
                        val saved = core.localDraft(id)
                        savedDraftIds = savedDraftIds + id
                        finishedDrafts.remove(id)
                        if (draft == null) draft = ComposeDraft(saved)
                        notice = null
                    } else { notice = "Already sent" }
                    if (undoSendId == id) undoSendId = null
                }
            }
        } finally { sending = false }
    }
}

internal fun Exception.mailMessage(): String = when (this) {
    is MailException.Failed -> reason
    is MailException.NotFound -> "It is no longer here."
    else -> message?.takeIf { it.isNotBlank() } ?: "Something went wrong. Try again."
}
