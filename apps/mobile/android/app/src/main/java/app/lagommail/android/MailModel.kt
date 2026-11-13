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
import java.time.Instant
import java.util.UUID

internal data class RowPresentation(val row: MailRow) {
    val sender = row.senderName.ifEmpty { row.senderAddress }
    val subject = row.subject.ifEmpty { "(No subject)" }
    val date: Instant = Instant.ofEpochSecond(row.dateUnix)
}
internal data class ComposeDraft(
    val account: String, val to: String = "", val cc: String = "", val subject: String = "",
    val body: String = "", val replyTo: Long? = null,
    val id: String = "<${UUID.randomUUID()}@draft.lagommail.app>",
    val attachments: List<SendAttachment> = emptyList(), val restored: Boolean = false,
    val isReply: Boolean = replyTo != null,
) {
    constructor(reply: ReplyDraft) : this(reply.account, reply.to, reply.cc, reply.subject, reply.body, reply.replyTo, attachments = reply.attachments)
    constructor(saved: LocalDraft) : this(saved.account, saved.to, saved.cc, saved.subject, saved.body,
        id = saved.id, attachments = saved.attachments, restored = true, isReply = saved.inReplyTo != null || saved.references.isNotEmpty())
    val hasContent get() = to.isNotEmpty() || cc.isNotEmpty() || subject.isNotEmpty() || body.isNotEmpty() || attachments.isNotEmpty()
}
internal enum class MailAction { Done, Later, Delete }

internal class MailModel(
    private val open: suspend () -> LagomCoreInterface,
    private val tomorrow: () -> Long = ::tomorrowAtEight,
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
    var drafts by mutableStateOf<List<LocalDraft>>(emptyList()); private set
    var showingDrafts by mutableStateOf(false); private set
    var savedDraftIds by mutableStateOf<Set<String>>(emptySet()); private set
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
        else if (!sending) draft?.let { persist(it) }
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
        val drafts = core.localDrafts()
        if (request != revision) return
        this.accounts = accounts
        this.failures = failures
        this.drafts = drafts
        savedDraftIds = savedDraftIds + drafts.map { it.id }
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
        showingDrafts = false
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
        val saved = core.savedReply(id)
        val prepared = if (saved != null) ComposeDraft(saved) else ComposeDraft(core.composeDraft(id, ComposeKind.REPLY))
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
    suspend fun openDraft(row: LocalDraft) = draftLock.withLock {
        val core = core ?: return@withLock
        if (draft != null) return@withLock
        val saved = core.localDrafts().firstOrNull { it.id == row.id } ?: return@withLock
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
                val saved = core.saveDraft(snapshot.id, snapshot.account, snapshot.to, snapshot.cc, snapshot.subject,
                    snapshot.body, snapshot.replyTo, snapshot.attachments)
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
                    val replacement = old.copy(id = "<${UUID.randomUUID()}@draft.lagommail.app>", account = account)
                    if (old.hasContent || old.id in savedDraftIds) {
                        // Save the replacement first so a failed account change cannot lose the original.
                        val saved = core.saveDraft(replacement.id, account, old.to, old.cc, old.subject, old.body, null, old.attachments)
                        drafts = listOf(saved) + drafts
                        savedDraftIds = savedDraftIds + saved.id
                        core.deleteDraft(old.id)
                    }
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
                    drafts = drafts.filterNot { it.id == snapshot.id }
                    savedDraftIds = savedDraftIds - snapshot.id
                    finishCompose(snapshot.id)
                    notice = "Queued for sending"
                }
            }
        } finally { sending = false }
        sync()
    }
}

internal fun Exception.mailMessage(): String = if (this is MailException.Failed) reason else message ?: "Mail operation failed"
