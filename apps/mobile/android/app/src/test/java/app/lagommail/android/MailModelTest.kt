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
        open = { core }, tomorrow = { 86_400 }, now = now)

    @Test fun editsDebounceAndKeepFlushesBeforeClosingAndRestores() = runTest(dispatcher) {
        val core = FakeCore()
        val model = model(core); model.start()
        model.draft = ComposeDraft("fixture", body = "first")
        advanceTimeBy(400)
        model.draft = model.draft!!.copy(body = "latest")
        advanceTimeBy(499); runCurrent()
        assertEquals(0, core.saveCalls)
        advanceTimeBy(1); runCurrent()
        assertEquals("latest", core.saved.values.single().body)
        model.draft = model.draft!!.copy(subject = "Last edit")
        model.closeCompose(discard = false)
        assertNull(model.draft)
        assertEquals("Last edit", core.saved.values.single().subject)
        val reopened = model(core); reopened.start(); reopened.showDrafts()
        reopened.openDraft(reopened.drafts.single())
        assertEquals("latest", reopened.draft?.body)
        assertEquals("Last edit", reopened.draft?.subject)
    }
    @Test fun discardWaitsForInflightAutosaveAndCannotResurrectDraft() = runTest(dispatcher) {
        val core = FakeCore()
        val model = model(core); model.start()
        model.draft = ComposeDraft("fixture", body = "saved")
        val gate = CompletableDeferred<Unit>(); core.saveGate = gate
        advanceTimeBy(500); runCurrent()
        assertEquals(1, core.saveCalls)
        val close = async { model.closeCompose(discard = true) }; runCurrent()
        assertFalse(close.isCompleted)
        gate.complete(Unit); close.await(); advanceUntilIdle()
        assertNull(model.draft)
        assertTrue(core.saved.isEmpty())
    }
    @Test fun saveAndDiscardFailuresKeepComposerAndEdits() = runTest(dispatcher) {
        val core = FakeCore()
        val model = model(core); model.start()
        model.draft = ComposeDraft("fixture", body = "keep me")
        core.rejectSave = true
        try { model.closeCompose(discard = false); fail("save should fail") } catch (_: IllegalStateException) { }
        assertEquals("keep me", model.draft?.body)
        core.rejectSave = false; core.rejectDelete = true
        try { model.closeCompose(discard = true); fail("discard should fail") } catch (_: IllegalStateException) { }
        assertEquals("keep me", model.draft?.body)
        assertFalse(model.sending)
    }
    @Test fun replyRestoresOnlyMatchingAccountAndSendConsumesSavedDraft() = runTest(dispatcher) {
        val core = FakeCore().apply { mail = listOf(row(42, account = "source")) }
        val model = model(core); model.start()
        core.saved["other"] = LocalDraft("other", "another", "other@example.test", "", "wrong", "wrong", "<42>", emptyList(), emptyList(), 1)
        model.reply(42)
        assertFalse(model.draft!!.restored)
        model.draft = model.draft!!.copy(body = "answer")
        model.closeCompose(discard = false)
        model.reply(42)
        assertTrue(model.draft!!.restored)
        assertEquals("answer", model.draft?.body)
        model.send()
        assertNull(model.draft)
        assertEquals("answer", core.sent.single().body)
        assertEquals(42L, core.sent.single().replyTo)
        assertEquals(setOf("other"), core.saved.keys)
    }
    @Test fun openingStaleDraftRowReadsLatestSavedBody() = runTest(dispatcher) {
        val core = FakeCore(); val model = model(core); model.start()
        model.draft = ComposeDraft("fixture", body = "before")
        model.closeCompose(discard = false)
        val stale = model.drafts.single()
        core.saved[stale.id] = stale.copy(body = "latest on disk")
        model.openDraft(stale)
        assertEquals("latest on disk", model.draft?.body)
        model.closeCompose(discard = true)
    }

    @Test fun changingSenderMovesEditsToNewIdentityAndClearedFieldsAutosave() = runTest(dispatcher) {
        val core = FakeCore(); val model = model(core); model.start()
        model.draft = ComposeDraft("fixture", body = "keep edits")
        advanceTimeBy(500); runCurrent()
        val old = model.draft!!.id
        model.changeComposeAccount("second")
        assertEquals("second", model.draft?.account)
        assertNotEquals(old, model.draft?.id)
        assertEquals("keep edits", core.saved.values.single().body)
        assertEquals("second", core.saved.values.single().account)
        model.draft = model.draft!!.copy(body = "")
        advanceTimeBy(500); runCurrent()
        assertEquals("", core.saved.values.single().body)
        model.closeCompose(discard = true)
    }

    @Test fun backgroundAndDismissalFlushTheLastEdit() = runTest(dispatcher) {
        val core = FakeCore(); val model = model(core); model.start()
        model.draft = ComposeDraft("fixture", body = "background")
        model.setForeground(false)
        assertEquals("background", core.saved.values.single().body)
        model.draft = model.draft!!.copy(body = "dismissed")
        model.draft = null
        runCurrent()
        assertEquals("dismissed", core.saved.values.single().body)
    }

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
        assertEquals(draft.account, core.sent.single().account)
        assertEquals(draft.body, core.sent.single().body)
        assertEquals(draft.replyTo, core.sent.single().replyTo)
    }
    @Test fun delayedReplyCannotOverwriteEditedCompose() = runTest(dispatcher) {
        val core = FakeCore().apply { mail = listOf(row(1)) }
        val model = model(core); model.start(); model.select(1)
        val gate = CompletableDeferred<Unit>(); core.replyGate = gate
        val reply = async { model.reply(1) }; runCurrent()
        model.draft = ComposeDraft("fixture")
        model.draft = model.draft!!.copy(body = "Unsent text")
        val edited = model.draft
        gate.complete(Unit); reply.await()
        assertEquals(edited, model.draft)
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
        assertEquals(draft.account, core.sent.single().account)
        assertEquals(draft.body, core.sent.single().body)
        assertEquals(draft.replyTo, core.sent.single().replyTo)
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

    val saved = mutableMapOf<String, LocalDraft>()
    val replyIds = mutableMapOf<String, Long?>()
    var saveCalls = 0
    var saveGate: CompletableDeferred<Unit>? = null
    var rejectSave = false
    var rejectDelete = false
    override suspend fun localDrafts() = saved.values.toList()
    override suspend fun savedReply(id: Long): LocalDraft? = saved.values.firstOrNull {
        it.account == mail.first { row -> row.id == id }.account && it.inReplyTo == "<${id}>"
    }
    override suspend fun saveDraft(id: String, accountId: String, to: String, cc: String, subject: String, body: String,
        replyTo: Long?, attachments: List<SendAttachment>): LocalDraft {
        saveCalls++
        saveGate?.await()
        check(!rejectSave) { "Fixture save refused" }
        replyIds[id] = replyTo ?: replyIds[id]
        val value = LocalDraft(id, accountId, to, cc, subject, body, replyIds[id]?.let { "<${it}>" }, emptyList(), attachments, 42)
        saved[id] = value
        return value
    }
    override suspend fun deleteDraft(id: String) { check(!rejectDelete) { "Fixture discard refused" }; saved.remove(id) }
    override suspend fun sendDraft(id: String): String {
        val draft = saved.getValue(id)
        val result = send(draft.account, listOf(draft.to), listOf(draft.cc), draft.subject, draft.body, replyIds[id], draft.attachments)
        saved.remove(id)
        return result
    }
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
        check(attachments.isEmpty())
        check(!rejectSend) { "Fixture enqueue refused" }
        sent += ComposeDraft(accountId, to.joinToString(","), cc.joinToString(","), subject, bodyMarkdown, replyTo)
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
        check(kind == ComposeKind.REPLY)
        replyGate?.await()
        return ReplyDraft(mail.first { it.id == id }.account, "sender@example.test", "", "Re: Subject", "\n\n-- \nSignature\n\n> body", id, emptyList())
    }
    override suspend fun addAccount(email: String, password: String) = error("unused fixture operation")
    override suspend fun addAccountManual(email: String, password: String, imap: MailServer, smtp: MailServer) = error("unused fixture operation")
}
