package app.lagommail.android

import app.lagommail.core.*
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.test.*
import org.junit.After
import org.junit.Assert.*
import org.junit.Before
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class MailModelTest {
    private val dispatcher = StandardTestDispatcher()
    @Before fun setup() { Dispatchers.setMain(dispatcher) }
    @After fun teardown() { Dispatchers.resetMain() }
    private fun model(core: FakeCore, now: () -> Long = { 0 }) = MailModel(
        open = { core }, tomorrow = { 86_400 }, recipients = { it.split(',').map(String::trim).filter(String::isNotEmpty) }, now = now)

    @Test fun refreshRetainsLoadedPagesAndScopeChangeResetsThem() = runTest(dispatcher) {
        val core = FakeCore().apply { mail = (1L..250L).reversed().map { row(it) } }
        val model = model(core)
        model.start()
        assertEquals(100, model.rows.size)
        model.reload(more = true)
        assertEquals(200, model.rows.size)
        model.sync()
        assertEquals(200, model.rows.size)
        model.reload(more = true)
        assertEquals(250, model.rows.size)
        assertFalse(model.hasMore)
        model.changeScope("other", Mailbox.SENT)
        assertTrue(model.rows.isEmpty())
        assertEquals("other", model.accountId)
        assertEquals(Mailbox.SENT, model.mailbox)
    }
    @Test fun staleReloadCannotReplaceNewScope() = runTest(dispatcher) {
        val core = FakeCore().apply { mail = listOf(row(1)) }
        val model = model(core); model.start()
        val gate = CompletableDeferred<Unit>()
        core.pageGate = gate
        val old = async { model.reload() }; runCurrent()
        core.pageGate = null
        model.changeScope("other", Mailbox.INBOX)
        gate.complete(Unit); old.await()
        assertTrue(model.rows.isEmpty())
    }
    @Test fun paginationDuringActionRefreshKeepsRemovedMailOutAndDoesNotSkipRows() = runTest(dispatcher) {
        val core = FakeCore().apply { mail = (1L..250L).reversed().map { row(it) } }
        val model = model(core); model.start()
        val gate = CompletableDeferred<Unit>(); core.pageGate = gate
        val action = async { model.act(MailAction.Done, 250) }; runCurrent()
        core.pageGate = null
        model.reload(more = true)
        gate.complete(Unit); action.await()
        assertEquals(core.mail.take(200), model.rows)
        model.reload(more = true)
        assertEquals(core.mail, model.rows)
        assertFalse(model.hasMore)
    }
    @Test fun messageReadsIncludingAfterMarkReadRunOffMain() = runTest(dispatcher) {
        val main = Thread.currentThread()
        val core = FakeCore().apply { mail = listOf(row(1, unread = true)) }
        val model = model(core); model.start()
        val gate = CompletableDeferred<Unit>(); core.messageGate = gate
        val started = CompletableDeferred<Unit>(); core.messageStarted = started
        val selection = model.perform { model.select(1) }
        started.await()
        gate.complete(Unit); selection.join()
        assertNull(model.error)
        assertEquals(4, core.messageThreads.size)
        assertTrue(core.messageThreads.all { it !== main })
        assertSame(main, core.markReadThread)
        assertFalse(model.message!!.row.unread)
    }
    @Test fun laterComputesDeadlineOffMainAndReturnsToMain() = runTest(dispatcher) {
        val main = Thread.currentThread()
        var deadlineThread: Thread? = null
        val core = FakeCore().apply { mail = listOf(row(1)) }
        val model = MailModel(open = { core }, tomorrow = {
            deadlineThread = Thread.currentThread()
            86_400
        })
        model.start()
        model.perform { model.act(MailAction.Later, 1) }.join()
        assertNull(model.error)
        assertNotNull(deadlineThread)
        assertNotSame(main, deadlineThread)
        assertSame(main, core.laterThread)
        assertEquals(listOf(1L) to 86_400L, core.snoozed)
        assertFalse(model.working)
    }
    @Test fun lateMessageCannotReplaceNewSelectionAndReadFailureKeepsCachedBody() = runTest(dispatcher) {
        val core = FakeCore().apply { mail = listOf(row(1), row(2, unread = true)) }
        val model = model(core); model.start()
        val gate = CompletableDeferred<Unit>()
        core.messageGate = gate
        val started = CompletableDeferred<Unit>(); core.messageStarted = started
        val first = async { model.select(1) }; started.await()
        core.messageGate = null
        core.rejectRead = true
        try { model.select(2); fail("expected refusal") } catch (_: IllegalStateException) { }
        gate.complete(Unit); first.await()
        assertEquals(2L, model.message?.row?.id)
        assertTrue(model.message!!.row.unread)
        assertEquals("body", model.message?.textBody)
    }
    @Test fun replyUsesCoreCompositionAndPreservesSourceAccountWhenSending() = runTest(dispatcher) {
        val core = FakeCore().apply { mail = listOf(row(42, account = "second-account")) }
        val model = model(core); model.start()
        model.reply(42)
        val draft = model.draft!!
        assertEquals("second-account", draft.account)
        assertEquals("sender@example.test", draft.to)
        assertEquals("", draft.cc)
        assertEquals("Re: Subject", draft.subject)
        assertEquals("\n\n-- \nSignature\n\n> body", draft.body)
        assertEquals(42L, draft.replyTo)
        model.send()
        assertEquals(draft, core.sent.single())
    }
    @Test fun delayedReplyCannotOverwriteEditedCompose() = runTest(dispatcher) {
        val core = FakeCore().apply { mail = listOf(row(1)) }
        val model = model(core); model.start(); model.select(1)
        val gate = CompletableDeferred<Unit>(); core.replyGate = gate
        val reply = async { model.reply(1, ComposeKind.FORWARD) }; runCurrent()
        model.draft = ComposeDraft("fixture")
        model.draft = model.draft!!.copy(body = "Unsent text")
        val edited = model.draft
        gate.complete(Unit); reply.await()
        assertEquals(edited, model.draft)
    }
    @Test fun replyAllAndForwardKeepPreparedFieldsAndAttachmentsThroughEditsAndRetry() = runTest(dispatcher) {
        val core = FakeCore().apply {
            mail = listOf(row(42, account = "second-account"))
            preparedAttachments = listOf(SendAttachment("report.txt", "text/plain", byteArrayOf(1, 2, 3)))
        }
        val model = model(core); model.start()
        for (kind in listOf(ComposeKind.REPLY_ALL, ComposeKind.FORWARD)) {
            model.reply(42, kind)
            val prepared = model.draft!!
            assertEquals(kind, core.composeKinds.last())
            assertEquals("second-account", prepared.account)
            assertEquals(if (kind == ComposeKind.FORWARD) "Forward" else "Reply all", prepared.title)
            assertEquals(if (kind == ComposeKind.FORWARD) null else 42L, prepared.replyTo)
            assertEquals(if (kind == ComposeKind.FORWARD) "" else "copy@example.test", prepared.cc)
            assertEquals(core.preparedAttachments, prepared.attachments)
            model.draft = prepared.copy(to = "recipient@example.test", body = "Edited quote")
            val edited = model.draft!!
            core.rejectSend = true
            try { model.send(); fail("expected refusal") } catch (_: IllegalStateException) { }
            assertEquals(edited, model.draft)
            core.rejectSend = false
            model.send()
            assertNull(model.draft)
            val sent = core.sent.last()
            assertEquals(edited.copy(title = sent.title), sent)
            assertArrayEquals(byteArrayOf(1, 2, 3), sent.attachments.single().data)
        }
    }
    @Test fun failedForwardPreparationDoesNotOpenAnIncompleteDraft() = runTest(dispatcher) {
        val core = FakeCore().apply { mail = listOf(row(1)); rejectCompose = true }
        val model = model(core); model.start()
        model.perform { model.reply(1, ComposeKind.FORWARD) }.join()
        assertNull(model.draft)
        assertEquals("Fixture attachment fetch refused", model.error)
        assertTrue(core.sent.isEmpty())
    }
    @Test fun delayedReplyStaysDismissedAfterComposeClosesOrSelectionChanges() = runTest(dispatcher) {
        val core = FakeCore().apply { mail = listOf(row(1), row(2)) }
        val model = model(core); model.start()
        for (navigate in listOf<suspend () -> Unit>(
            { model.draft = ComposeDraft("fixture"); model.draft = null },
            { model.clearSelection() },
            { model.select(2) },
            { model.changeScope(null, Mailbox.SENT) },
        )) {
            model.select(1)
            val gate = CompletableDeferred<Unit>(); core.replyGate = gate
            val reply = async { model.reply(1) }; runCurrent()
            navigate()
            gate.complete(Unit); reply.await()
            assertNull(model.draft)
        }
    }
    @Test fun newestReplyWinsEvenIfOlderRequestFinishesLast() = runTest(dispatcher) {
        val core = FakeCore().apply { mail = listOf(row(1), row(2)) }
        val model = model(core); model.start()
        val gate = CompletableDeferred<Unit>(); core.replyGate = gate
        val first = async { model.reply(1) }; runCurrent()
        core.replyGate = null
        model.reply(2)
        gate.complete(Unit); first.await()
        assertEquals(2L, model.draft?.replyTo)
    }
    @Test fun sendDismissesOnlyAfterDurableEnqueueAndDoesNotRequeueOnSyncFailure() = runTest(dispatcher) {
        val core = FakeCore()
        val model = model(core); model.start()
        val draft = ComposeDraft("fixture", "to@example.test", "cc@example.test", "Subject", "**body**", 42)
        model.draft = draft
        core.rejectSend = true
        try { model.send(); fail("expected refusal") } catch (_: IllegalStateException) { }
        assertEquals(draft, model.draft)
        assertFalse(model.sending)
        core.rejectSend = false; core.rejectSync = true
        model.send()
        assertNull(model.draft)
        assertEquals(1, core.sent.size)
        assertEquals(draft, core.sent.single())
        assertNotNull(model.error)
        model.sync()
        assertEquals(1, core.sent.size)
    }
    @Test fun cachedReadsAndSendRemainAvailableDuringSync() = runTest(dispatcher) {
        val core = FakeCore().apply { mail = listOf(row(1)) }
        val model = model(core); model.start()
        val gate = CompletableDeferred<Unit>(); core.syncGate = gate
        val sync = async { model.sync() }; runCurrent()
        model.select(1)
        model.draft = ComposeDraft("fixture", "to@example.test")
        model.send()
        assertNull(model.draft)
        assertEquals(1L, model.message?.row?.id)
        assertEquals(1, core.sent.size)
        core.syncGate = null; gate.complete(Unit); sync.await()
        assertFalse(model.syncing)
    }
    @Test fun laterWakesInForegroundAndStopsInBackground() = runTest(dispatcher) {
        val core = FakeCore().apply { due = 10 }
        val model = model(core) { testScheduler.currentTime }
        model.setForeground(true)
        val initial = core.pageCalls
        core.due = null
        advanceTimeBy(10_001); runCurrent()
        assertTrue(core.pageCalls > initial)
        core.due = 20
        model.reload()
        model.setForeground(false)
        val stopped = core.pageCalls
        advanceTimeBy(20_000); runCurrent()
        assertEquals(stopped, core.pageCalls)
    }
    @Test fun actionsKeepSelectionOnFailureAndUseCoreSnoozeDeadline() = runTest(dispatcher) {
        val core = FakeCore().apply { mail = listOf(row(1)) }
        val model = model(core); model.start(); model.select(1)
        core.rejectAction = true
        try { model.act(MailAction.Done, 1); fail("expected refusal") } catch (_: IllegalStateException) { }
        assertEquals(1L, model.selectedId)
        assertFalse(model.working)
        core.rejectAction = false
        model.act(MailAction.Later, 1)
        assertEquals(listOf(1L) to 86_400L, core.snoozed)
        assertNull(model.selectedId)
    }
}

private class FakeCore : LagomCoreInterface {
    override suspend fun inlineResource(id: Long, request: String): InlineResource = error("unused")
    override suspend fun loadInline(id: Long): Unit = error("unused")
    override suspend fun search(accountId: String?, query: String, limit: UInt, offset: UInt): List<MailRow> = error("unused")
    // Planner screens are iOS-only so far; the mail model never calls these.
    override suspend fun addTodo(list: Long, title: String, dueDate: String?): Todo = error("unused")
    override suspend fun addTodoList(name: String): TodoList = error("unused")
    override suspend fun agenda(startDay: String, days: UInt): List<AgendaItem> = emptyList()
    override suspend fun calendarConnected(): Boolean = false
    override suspend fun deleteTodo(id: Long) {}
    override suspend fun mailCounts(): List<MailCounts> = emptyList()
    override suspend fun setTodoDone(id: Long, done: Boolean) {}
    override suspend fun today(unreadLimit: UInt): TodaySummary = error("unused")
    override suspend fun todoLists(): List<TodoList> = emptyList()
    override suspend fun todos(list: Long, includeDone: Boolean): List<Todo> = emptyList()

    var mail = emptyList<MailRow>()
    var due: Long? = null
    var pageCalls = 0
    var pageGate: CompletableDeferred<Unit>? = null
    var messageGate: CompletableDeferred<Unit>? = null
    var messageStarted: CompletableDeferred<Unit>? = null
    val messageThreads = mutableListOf<Thread>()
    var markReadThread: Thread? = null
    var laterThread: Thread? = null
    var replyGate: CompletableDeferred<Unit>? = null
    var preparedAttachments = emptyList<SendAttachment>()
    val composeKinds = mutableListOf<ComposeKind>()
    var rejectCompose = false
    var syncGate: CompletableDeferred<Unit>? = null
    var rejectRead = false
    var rejectSend = false
    var rejectSync = false
    var rejectAction = false
    val sent = mutableListOf<ComposeDraft>()
    var snoozed: Pair<List<Long>, Long>? = null
    override suspend fun accounts() = listOf(MailAccount("fixture", "fixture@example.test", "Fixture"))
    override suspend fun mailbox(accountId: String?, mailbox: Mailbox, limit: UInt, offset: UInt): List<MailRow> {
        pageCalls++
        val snapshot = mail.filter { accountId == null || accountId == it.account }.drop(offset.toInt()).take(limit.toInt())
        pageGate?.await()
        return snapshot
    }
    override suspend fun inbox(limit: UInt, offset: UInt) = mailbox(null, Mailbox.INBOX, limit, offset)
    override suspend fun nextSnooze() = due
    override suspend fun sendFailures() = emptyList<SendFailure>()
    override suspend fun syncAll() { syncGate?.await(); check(!rejectSync) { "Fixture sync refused" } }
    override suspend fun sync(accountId: String) = syncAll()
    override suspend fun message(id: Long): MailMessage {
        messageThreads += Thread.currentThread()
        val result = MailMessage(mail.first { it.id == id }, "body", null, emptyList())
        val gate = messageGate
        messageStarted?.complete(Unit)
        gate?.await()
        messageThreads += Thread.currentThread()
        return result
    }
    override suspend fun markRead(id: Long, read: Boolean) {
        markReadThread = Thread.currentThread()
        check(!rejectRead) { "Fixture read refused" }
        mail = mail.map { if (it.id == id) it.copy(unread = !read) else it }
    }
    override suspend fun send(accountId: String, to: List<String>, cc: List<String>, subject: String, bodyMarkdown: String, replyTo: Long?, attachments: List<SendAttachment>): String {
        check(!rejectSend) { "Fixture enqueue refused" }
        sent += ComposeDraft(accountId, to.joinToString(","), cc.joinToString(","), subject, bodyMarkdown, replyTo, attachments)
        return "queued"
    }
    override suspend fun later(ids: List<Long>, untilUnix: Long) {
        laterThread = Thread.currentThread()
        check(!rejectAction); snoozed = ids to untilUnix
    }
    override suspend fun done(ids: List<Long>) { check(!rejectAction); mail = mail.filterNot { it.id in ids } }
    override suspend fun delete(ids: List<Long>) { check(!rejectAction) }
    override suspend fun removeAccount(id: String) { mail = mail.filterNot { it.account == id } }
    override suspend fun retrySend(id: String) = Unit
    override suspend fun replyDraft(id: Long): ReplyDraft = error("Reply must use composeDraft")
    override suspend fun composeDraft(id: Long, kind: ComposeKind): ReplyDraft {
        composeKinds += kind
        replyGate?.await()
        check(!rejectCompose) { "Fixture attachment fetch refused" }
        return ReplyDraft(mail.first { it.id == id }.account,
            if (kind == ComposeKind.FORWARD) "" else "sender@example.test",
            if (kind == ComposeKind.REPLY_ALL) "copy@example.test" else "",
            if (kind == ComposeKind.FORWARD) "Fwd: Subject" else "Re: Subject",
            "\n\n-- \nSignature\n\n> body", if (kind == ComposeKind.FORWARD) null else id, preparedAttachments)
    }
    override suspend fun addAccount(email: String, password: String) = error("unused fixture operation")
    override suspend fun addAccountManual(email: String, password: String, imap: MailServer, smtp: MailServer) = error("unused fixture operation")
}
