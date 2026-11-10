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
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.time.Instant

internal data class RowPresentation(val row: MailRow) {
    val sender = row.senderName.ifEmpty { row.senderAddress }
    val subject = row.subject.ifEmpty { "(No subject)" }
    val date: Instant = Instant.ofEpochSecond(row.dateUnix)
}
internal data class ComposeDraft(
    val account: String, val to: String = "", val cc: String = "", val subject: String = "",
    val body: String = "", val replyTo: Long? = null,
) {
    constructor(reply: ReplyDraft) : this(reply.account, reply.to, "", reply.subject, reply.body, reply.replyTo)
}
internal enum class MailAction { Done, Later, Delete }

internal class MailModel(
    private val open: suspend () -> LagomCoreInterface,
    private val tomorrow: () -> Long = ::tomorrowAtEight,
    private val recipients: (String) -> List<String> = ::splitRecipients,
    private val now: () -> Long = { System.currentTimeMillis() },
) : ViewModel() {
    var core: LagomCoreInterface? = null; private set
    var accounts by mutableStateOf<List<MailAccount>>(emptyList()); private set
    var rows by mutableStateOf<List<MailRow>>(emptyList()); private set
    var failures by mutableStateOf<List<SendFailure>>(emptyList()); private set
    var accountId by mutableStateOf<String?>(null); private set
    var mailbox by mutableStateOf(Mailbox.INBOX); private set
    var selectedId by mutableStateOf<Long?>(null); private set
    var message by mutableStateOf<MailMessage?>(null); private set
    var hasMore by mutableStateOf(false); private set
    var syncing by mutableStateOf(false); private set
    var working by mutableStateOf(false); private set
    var sending by mutableStateOf(false); private set
    var loading by mutableStateOf(true); private set
    var error by mutableStateOf<String?>(null)
    var notice by mutableStateOf<String?>(null)
    // shortcut: compose lives in memory only, add Rust draft persistence before process-death restoration.
    private var composeRevision = 0
    private var composeDraft by mutableStateOf<ComposeDraft?>(null)
    var draft: ComposeDraft?
        get() = composeDraft
        set(value) { composeRevision++; composeDraft = value }
    var accountForm by mutableStateOf<AccountForm?>(null)
    private var revision = 0
    private var appliedRevision = 0
    private var selectionRevision = 0
    private var syncAgain = false
    private var foreground = false
    private var wake: Job? = null

    fun perform(action: suspend () -> Unit) = viewModelScope.launch {
        try { action() } catch (e: CancellationException) { throw e }
        catch (e: Exception) { error = e.mailMessage() }
    }
    suspend fun start() {
        if (core != null) return
        try { core = open(); reload() } finally { loading = false }
    }
    suspend fun setForeground(active: Boolean) {
        foreground = active
        wake?.cancel(); wake = null
        if (active) { start(); reload(); sync() }
    }
    suspend fun reload(more: Boolean = false) {
        val core = core ?: return
        // Pending reloads may have changed the rows before the pagination offset.
        val append = more && revision == appliedRevision
        val request = ++revision
        val old = rows
        val offset = if (append) old.size else 0
        val account = accountId
        val box = mailbox
        val due = core.nextSnooze()
        val accounts = core.accounts()
        val page = mutableListOf<MailRow>()
        val count = maxOf(100, old.size + if (more) 100 else 0) - offset
        var hasMore: Boolean
        do {
            val batch = core.mailbox(account, box, 100u, (offset + page.size).toUInt())
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
        wake?.cancel()
        wake = if (foreground && due != null) viewModelScope.launch {
            delay(maxOf(1, due * 1000 - now()))
            wake = null
            try { reload() } catch (e: CancellationException) { throw e }
            catch (e: Exception) { error = e.mailMessage() }
        } else null
    }
    suspend fun changeScope(account: String?, box: Mailbox) {
        revision++
        clearSelection()
        accountId = account; mailbox = box; rows = emptyList(); hasMore = false
        reload()
    }
    fun clearSelection() { selectionRevision++; selectedId = null; message = null }
    suspend fun select(id: Long) {
        val core = core ?: return
        val request = ++selectionRevision
        selectedId = id; message = null
        try {
            val loaded = withContext(Dispatchers.IO) { core.message(id) }
            if (request != selectionRevision) return
            message = loaded
            if (loaded.row.unread) {
                core.markRead(id, true)
                if (request != selectionRevision) return
                val read = withContext(Dispatchers.IO) { core.message(id) }
                if (request != selectionRevision) return
                message = read
                reload()
            }
        } catch (e: CancellationException) { throw e }
        catch (e: Exception) { if (request == selectionRevision) throw e }
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
    suspend fun act(action: MailAction, id: Long) {
        val core = core ?: return
        if (working) return
        working = true
        try {
            when (action) {
                MailAction.Done -> core.done(listOf(id))
                MailAction.Later -> core.later(listOf(id), withContext(Dispatchers.IO) { tomorrow() })
                MailAction.Delete -> core.delete(listOf(id))
            }
            if (selectedId == id) clearSelection()
        } finally { working = false; reload() }
    }
    suspend fun remove(account: MailAccount) {
        core?.removeAccount(account.id)
        changeScope(if (accountId == account.id) null else accountId, mailbox)
    }
    suspend fun reply(id: Long) {
        val core = core ?: return
        if (draft != null) return
        val request = ++composeRevision
        val selection = selectionRevision
        val reply = core.replyDraft(id)
        if (request == composeRevision && selection == selectionRevision) draft = ComposeDraft(reply)
    }
    suspend fun send() {
        val core = core ?: return
        val draft = draft ?: return
        if (sending) return
        sending = true
        try {
            core.send(draft.account, recipients(draft.to), recipients(draft.cc), draft.subject, draft.body, draft.replyTo)
            this.draft = null
            notice = "Queued for sending"
        } finally { sending = false }
        sync()
    }
}

internal fun Exception.mailMessage(): String = if (this is MailException.Failed) reason else message ?: "Mail operation failed"
