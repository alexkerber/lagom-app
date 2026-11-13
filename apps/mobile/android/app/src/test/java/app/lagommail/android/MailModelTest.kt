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
import org.junit.Rule
import org.junit.rules.TemporaryFolder
import java.io.File
import java.util.UUID

@OptIn(ExperimentalCoroutinesApi::class)
class MailModelTest {
    @get:Rule val temporary = TemporaryFolder()
    private val dispatcher = StandardTestDispatcher()
    @Before fun setup() { Dispatchers.setMain(dispatcher) }
    @After fun teardown() { Dispatchers.resetMain() }
    private var sendLaters = 0
    private fun model(core: FakeCore, now: () -> Long = { 0 }) = MailModel(
        open = { core }, attachmentDirectory = File(temporary.newFolder(), "attachments"), now = now, sendLater = { sendLaters++ })

    @Test fun undoRestoresDraftAndExpirySyncsOnlyInForeground() = runTest(dispatcher) {
        val core = FakeCore(); val model = model(core); model.setForeground(true)
        val initialSyncs = core.syncs
        val draft = ComposeDraft("fixture", to = "to@example.test", body = "keep edits",
            attachments = listOf(SendAttachment("a.bin", "application/octet-stream", byteArrayOf(1, 2))))
        model.draft = draft; model.send(); runCurrent()
        assertEquals("Sending…", model.notice); assertEquals(draft.id, model.undoSendId)
        advanceTimeBy(4999); runCurrent()
        assertEquals(initialSyncs, core.syncs)
        model.undoSend()
        assertEquals(draft.id, model.draft?.id); assertEquals(draft.body, model.draft?.body)
        assertEquals(draft.attachments, model.draft?.attachments); assertNull(model.undoSendId)
        advanceTimeBy(1); runCurrent()
        assertEquals(initialSyncs + 1, core.syncs)
        model.send(); runCurrent(); core.queued.clear(); model.undoSend()
        assertEquals("Already sent", model.notice); assertNull(model.draft)
        model.setForeground(false)
        assertEquals(0, sendLaters)
        advanceTimeBy(5000); runCurrent()
        assertEquals(initialSyncs + 1, core.syncs); assertEquals(1, sendLaters)
        model.setForeground(true)
        assertEquals(initialSyncs + 2, core.syncs)
        model.draft = draft.copy(id = "next"); model.send(); runCurrent()
        advanceTimeBy(5000); runCurrent()
        assertNull(model.undoSendId); assertNull(model.notice)
        assertEquals(initialSyncs + 3, core.syncs)
    }

    @Test fun leavingInsideUndoWindowHandsSendToWorkManager() = runTest(dispatcher) {
        val core = FakeCore(); val model = model(core); model.setForeground(true)
        model.draft = ComposeDraft("fixture", to = "to@example.test", body = "leave now"); model.send(); runCurrent()
        model.setForeground(false)
        assertEquals(1, sendLaters)
        model.setForeground(true); model.setForeground(false)
        assertEquals(2, sendLaters)
    }

    @Test fun resendingRestoredDraftGetsItsOwnFullUndoWindow() = runTest(dispatcher) {
        val core = FakeCore(); val model = model(core); model.setForeground(true)
        model.draft = ComposeDraft("fixture", to = "to@example.test", body = "same Message-ID")
        model.send(); runCurrent()
        advanceTimeBy(1000); model.undoSend()
        val id = model.draft!!.id
        model.send(); runCurrent()
        advanceTimeBy(4000); runCurrent()
        assertEquals(id, model.undoSendId); assertEquals("Sending…", model.notice)
        advanceTimeBy(1000); runCurrent()
        assertNull(model.undoSendId); assertNull(model.notice)
    }

    @Test fun undoPreservesAnEditorOpenedDuringRestoration() = runTest(dispatcher) {
        val core = FakeCore(); val model = model(core); model.start()
        val original = ComposeDraft("fixture", to = "to@example.test", body = "original")
        model.draft = original; model.send()
        val newer = ComposeDraft("fixture", body = "new editor")
        core.onRestore = { model.draft = newer }
        model.undoSend()
        assertEquals(newer, model.draft)
        assertEquals(original.body, core.saved[original.id]?.body)
        core.onRestore = null
    }

    @Test fun laterUsesEveryCoreChoiceAndTomorrowForSwipe() = runTest(dispatcher) {
        val core = FakeCore().apply { mail = listOf(row(1)) }; val model = model(core); model.start()
        for (choice in core.laterChoices()) {
            model.act(MailAction.Later, 1, until = choice.untilUnix)
            assertEquals(listOf(1L) to choice.untilUnix, core.snoozed)
        }
        model.act(MailAction.Later, 1)
        assertEquals(listOf(1L) to 86_400L, core.snoozed)
    }

    @Test fun conversationExpansionReadStateAndRemoteConsentArePerMessage() = runTest(dispatcher) {
        val core = FakeCore().apply {
            mail = listOf(row(1), row(2, unread = true), row(3))
            conversationIds = listOf(1, 2, 3)
        }
        val model = model(core); model.start(); model.select(3)
        assertEquals(listOf(1L, 2L, 3L), model.messages.map { it.row.id })
        assertEquals(setOf(2L, 3L), model.expandedIds)
        assertEquals(listOf(2L to true), core.readCalls)
        assertFalse(model.messages[1].row.unread)
        model.loadRemoteImages(2)
        model.toggleMessage(2)
        assertEquals(setOf(3L), model.expandedIds)
        model.setRead(2, false); model.toggleMessage(2)
        assertFalse(model.messages[1].row.unread)
        assertEquals(setOf(2L), model.remoteImagesIds)
        model.toggleMessage(1)
        assertEquals(setOf(1L, 2L, 3L), model.expandedIds)
        assertEquals(listOf(2L to true, 2L to false, 2L to true), core.readCalls)
        model.reply(1, ComposeKind.REPLY_ALL)
        assertEquals(1L, model.draft?.replyTo)
        model.clearSelection()
        assertTrue(model.messages.isEmpty()); assertTrue(model.expandedIds.isEmpty()); assertTrue(model.remoteImagesIds.isEmpty())
    }
    @Test fun listActionsUseVisibleThreadMembers() = runTest(dispatcher) {
        val core = FakeCore().apply { mail = listOf(row(3).copy(messageCount = 2u, messageIds = listOf(3, 1))) }
        val model = model(core); model.start()
        model.act(MailAction.Later, 3)
        assertEquals(listOf(3L, 1L) to 86_400L, core.snoozed)
        model.act(MailAction.Delete, 3)
        assertEquals(listOf(3L, 1L), core.deleted)
    }

    @Test fun selectedThreadActionSurvivesRepresentativeChange() = runTest(dispatcher) {
        val core = FakeCore().apply { mail = listOf(row(1), row(3)) }
        val model = model(core); model.start(); model.select(1)
        core.mail = listOf(row(2).copy(messageCount = 2u, messageIds = listOf(2, 1)), row(3))
        model.sync()
        assertEquals(1L, model.selectedId)
        assertEquals(listOf(2L, 3L), model.rows.map { it.id })
        model.act(MailAction.Done, model.selectedId!!)
        assertEquals(listOf(3L), core.mail.map { it.id })
        assertNull(model.selectedId)
    }

    @Test fun explicitlyOpenedReadMemberStartsExpanded() = runTest(dispatcher) {
        val core = FakeCore().apply {
            mail = listOf(row(1), row(2), row(3, unread = true), row(4))
            conversationIds = listOf(1, 2, 3, 4)
        }
        val model = model(core); model.start(); model.select(1)
        assertEquals(1L, model.message?.row?.id)
        assertEquals(setOf(1L, 3L, 4L), model.expandedIds)
        assertEquals(listOf(3L to true), core.readCalls)
    }

    @Test fun notificationContentGroupsAccountsAndTargetsNewestMail() {
        val first = row(1, "a").copy(senderName = "", senderAddress = "sender@example.test", subject = "First", dateUnix = 1)
        assertTrue(mailNotices(emptyList()).isEmpty())
        assertEquals(listOf(MailNotice(MailTarget("a", 1), "sender@example.test", "First")), mailNotices(listOf(first)))
        val named = first.copy(account = "b", senderName = "Sender")
        assertEquals("Sender", mailNotices(listOf(named)).single().title)
        val second = first.copy(id = 2, dateUnix = 2, subject = "Second")
        val third = second.copy(id = 3, subject = "Third")
        val fourth = third.copy(id = 4, subject = "Fourth")
        val notices = mailNotices(listOf(first, named, second, fourth, third))
        assertEquals(2, notices.size)
        assertEquals(MailNotice(MailTarget("a", 4), "4 new messages", "Fourth\nThird\nSecond"), notices[0])
    }

    @Test fun notificationSelectsAccountAndMessageOutsideCurrentSearchAndMailbox() = runTest(dispatcher) {
        val core = FakeCore().apply { mail = listOf(row(42, "second")) }
        val model = model(core)
        model.start()
        model.changeScope("fixture", Mailbox.SENT)
        model.changeSearch("old search")
        model.openNotification(MailTarget("second", 42))
        assertEquals("second", model.accountId)
        assertEquals(Mailbox.INBOX, model.mailbox)
        assertEquals("", model.searchText)
        assertEquals(42L, model.selectedId)
        assertEquals(42L, model.message?.row?.id)
    }

    @Test fun recipientTokensUseDesktopBoundariesAndPreserveEarlierAddresses() {
        assertEquals("", recipientToken(""))
        assertEquals("", recipientToken("  "))
        assertEquals("An", recipientToken("An"))
        assertEquals("An", recipientToken("one@example.com,  An "))
        assertEquals("An", recipientToken("one@example.com;\r\nAn"))
        assertEquals("", recipientToken("one@example.com, "))
        assertEquals("An", recipientToken("\"Team, West\" <west@example.com>, An"))
        assertEquals("\"Team, West", recipientToken("\"Team, West"))
        assertEquals("An", recipientToken("\"Team\\\", West\" <west@example.com>; An"))
        assertEquals("An", recipientToken("One (team, (west; east)) <one@example.com>, An"))
        assertEquals("odd <one,two", recipientToken("odd <one,two"))
        assertEquals("Éq", recipientToken("Åsa@example.com, Éq"))
        assertEquals("\"Team, West\" <west@example.com>, ", recipientToken("An", "\"Team, West\" <west@example.com>"))
        assertEquals("one@example.com,  \"Team, West\" <west@example.com>, ", recipientToken("one@example.com,  An", "\"Team, West\" <west@example.com>"))
        assertEquals("\"Team, West\" <west@example.com>, \"Team, West\" <west@example.com>, ", recipientToken("\"Team, West\" <west@example.com>, We", "\"Team, West\" <west@example.com>"))
        assertEquals("one@example.com;\r\n\"Team, West\" <west@example.com>, ", recipientToken("one@example.com;\r\nWe", "\"Team, West\" <west@example.com>"))
        assertEquals("\"Team, West\" <west@example.com>, ", recipientToken("\"Team, W", "\"Team, West\" <west@example.com>"))
    }

    @Test fun safeAttachmentNames() {
        for ((input, expected) in listOf("../../secret.txt" to "secret.txt", "..\\..\\safe.pdf" to "safe.pdf",
            "" to "attachment", " . " to "attachment", ".." to "attachment", "dir/" to "attachment",
            " \u0000\n\t " to "attachment", " a\u0000b\u007fc.txt " to "abc.txt")) {
            assertEquals(expected, safeAttachmentName(input))
        }
        assertEquals(120, safeAttachmentName("x".repeat(121)).length)
    }
    @Test fun attachmentClearsOldCacheWritesBytesAndBlocksDoubleTaps() = runTest(dispatcher) {
        val directory = temporary.newFolder("attachments")
        val old = File(directory, "old.txt").apply { writeText("old") }
        val core = FakeCore()
        val model = MailModel(open = { core }, attachmentDirectory = directory)
        model.start()
        assertFalse(old.exists())
        val part = MailAttachment("2", "../../fixture.bin", "application/octet-stream", 2u)
        val gate = CompletableDeferred<Unit>(); core.attachmentGate = gate
        val pending = async { model.openAttachment(42, part) }; runCurrent()
        assertEquals(setOf(42L to "2"), model.downloadingAttachments)
        assertNull(model.openAttachment(42, part))
        assertEquals(listOf(42L to "2"), core.attachmentCalls)
        gate.complete(Unit)
        val file = pending.await()!!
        assertArrayEquals(byteArrayOf(0, -1), file.readBytes())
        assertEquals("fixture.bin", file.name)
        assertNotNull(UUID.fromString(file.parentFile!!.name))
        assertEquals(directory, file.parentFile!!.parentFile)
        assertTrue(model.downloadingAttachments.isEmpty())
        assertNotEquals(file, model.openAttachment(42, part))
        core.rejectAttachment = true
        model.perform { model.openAttachment(42, part) }.join()
        assertEquals("download failed", model.error)
        assertTrue(model.downloadingAttachments.isEmpty())
    }
    @Test fun attachmentCannotOpenAfterChangingMessage() = runTest(dispatcher) {
        val core = FakeCore(); val model = model(core); model.start()
        val gate = CompletableDeferred<Unit>(); core.attachmentGate = gate
        val pending = async { model.openAttachment(42, MailAttachment("2", "file.txt", "text/plain", 2u)) }; runCurrent()
        model.clearSelection()
        gate.complete(Unit)
        assertNull(pending.await())
        assertTrue(model.downloadingAttachments.isEmpty())
    }
    @Test fun messageIdUsesLastDomainAndCompactLowercaseUuid() {
        for ((email, domain) in listOf("name@earlier@example.test" to "example.test", "missing" to "localhost")) {
            val id = draftMessageId(email)
            assertTrue(id.matches(Regex("<[0-9a-f]{32}@${Regex.escape(domain)}>")))
            assertNotEquals(id, draftMessageId(email))
        }
    }
    @Test fun inboxReloadSurvivesDraftListFailure() = runTest(dispatcher) {
        val core = FakeCore().apply { mail = listOf(row(1)); rejectDraftList = true }
        val model = model(core); model.start()
        assertEquals(listOf(1L), model.rows.map { it.id })
        assertNotNull(model.error)
    }
    @Test fun discardDuringDraftListLoadDropsStaleRows() = runTest(dispatcher) {
        val core = FakeCore().apply { mail = listOf(row(1)) }
        val model = model(core); model.start()
        model.draft = ComposeDraft("fixture", body = "saved")
        model.closeCompose(discard = false)
        model.openDraft(model.drafts.single())
        val gate = CompletableDeferred<Unit>(); core.draftListGate = gate
        val load = async { model.reload() }; runCurrent()
        assertFalse(load.isCompleted)
        model.closeCompose(discard = true)
        gate.complete(Unit); load.await()
        assertTrue(model.drafts.isEmpty())
        assertTrue(model.savedDraftIds.isEmpty())
        assertEquals(listOf(1L), model.rows.map { it.id })
    }
    @Test fun openingDeletedDraftRemovesListRow() = runTest(dispatcher) {
        val core = FakeCore(); val model = model(core); model.start()
        model.draft = ComposeDraft("fixture", body = "saved")
        model.closeCompose(discard = false)
        val row = model.drafts.single()
        core.saved.clear()
        model.openDraft(row)
        assertTrue(model.drafts.isEmpty())
        assertNull(model.draft)
        assertNull(model.error)
    }

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
        core.saved["other"] = LocalDraft("other", "another", "other@example.test", "", "wrong", "wrong", "<42>", emptyList(), emptyList())
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
        core.saved[stale.id] = core.saved.getValue(stale.id).copy(body = "latest on disk")
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
    @Test fun notificationDoesNotOverrideNavigationDuringItsReload() = runTest(dispatcher) {
        val core = FakeCore().apply { mail = listOf(row(42, "second")) }
        val model = model(core); model.start()
        val gate = CompletableDeferred<Unit>(); core.pageGate = gate
        val open = async { model.openNotification(MailTarget("second", 42)) }; runCurrent()
        core.pageGate = null
        model.changeScope("second", Mailbox.SENT)
        model.changeScope("second", Mailbox.INBOX)
        gate.complete(Unit); open.await()
        assertNull(model.selectedId)
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
    @Test fun searchPagesCachedMailAcrossAccountsAndResetsOnQueryAndScopeChanges() = runTest(dispatcher) {
        val core = FakeCore().apply {
            mail = (1L..250L).map { row(it, account = if (it % 2L == 0L) "second" else "fixture").copy(subject = "Meeting") } +
                row(251).copy(subject = "Receipt")
        }
        val model = model(core); model.start(); model.select(1)
        model.changeSearch("  Meeting \n")
        assertNull(model.selectedId)
        assertEquals(100, model.rows.size)
        assertEquals(Triple(null, "Meeting", 0u), core.searchCalls.last())
        model.reload(more = true)
        assertEquals(200, model.rows.size)
        model.sync()
        assertEquals(200, model.rows.size)
        model.reload(more = true)
        assertEquals(250, model.rows.size)
        assertFalse(model.hasMore)
        model.changeScope("second", Mailbox.SENT)
        assertEquals(100, model.rows.size)
        assertEquals(Triple("second", "Meeting", 0u), core.searchCalls.last())
        assertTrue(model.rows.all { it.account == "second" })
        model.reload(more = true)
        assertEquals(125, model.rows.size)
        model.changeSearch("Receipt")
        assertTrue(model.rows.isEmpty())
        model.changeScope(null, Mailbox.INBOX)
        assertEquals(listOf(251L), model.rows.map { it.id })
        val searches = core.searchCalls.size
        model.changeSearch(" \n")
        assertEquals(100, model.rows.size)
        assertEquals(searches, core.searchCalls.size)
        assertTrue(model.hasMore)
    }
    @Test fun staleSearchCannotReplaceAnotherQueryOrAccountOrClearedSearch() = runTest(dispatcher) {
        val core = FakeCore().apply { mail = listOf(row(1).copy(subject = "Old"), row(2).copy(subject = "New")) }
        val model = model(core); model.start()
        for (change in listOf<suspend () -> Unit>(
            { model.changeSearch("New") },
            { model.changeScope("other", Mailbox.INBOX) },
            { model.changeSearch("") },
        )) {
            model.changeScope(null, Mailbox.INBOX); model.changeSearch("")
            val gate = CompletableDeferred<Unit>(); core.searchGate = gate
            val old = async { model.changeSearch("Old") }; runCurrent()
            core.searchGate = null
            change()
            val expected = model.rows
            gate.complete(Unit); old.await()
            assertEquals(expected, model.rows)
        }
    }
    @Test fun unreadFilterKeepsUnfilteredPagingOffsetsAndLoadMoreAvailable() = runTest(dispatcher) {
        val core = FakeCore().apply { mail = (1L..150L).map { row(it, unread = it > 100).copy(subject = "Match") } }
        val model = model(core); model.start()
        model.changeSearch("Match"); model.unreadOnly = true
        assertTrue(model.visibleRows.isEmpty())
        assertTrue(model.hasMore)
        model.reload(more = true)
        assertEquals(Triple(null, "Match", 100u), core.searchCalls.last())
        assertEquals((101L..150L).toList(), model.visibleRows.map { it.id })
        assertFalse(model.hasMore)
        model.unreadOnly = false
        assertEquals(core.mail, model.visibleRows)
    }
    @Test fun staleSearchErrorsAreIgnoredButCurrentErrorsAreReported() = runTest(dispatcher) {
        val core = FakeCore()
        val model = model(core); model.start()
        core.rejectSearch = true
        val gate = CompletableDeferred<Unit>(); core.searchGate = gate
        val old = model.perform { model.changeSearch("Old") }; runCurrent()
        model.changeSearch("")
        gate.complete(Unit); old.join()
        assertNull(model.error)
        model.perform { model.changeSearch("New") }.join()
        assertEquals("Fixture search refused", model.error)
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
    @Test fun explicitReadStateUsesConfirmedFlagsAndRetainsMessageOnFailure() = runTest(dispatcher) {
        val core = FakeCore().apply { mail = listOf(row(1)) }
        val model = model(core); model.start(); model.select(1); model.unreadOnly = true
        model.setRead(1, false)
        assertTrue(model.message!!.row.unread)
        assertEquals(listOf(1L), model.visibleRows.map { it.id })
        core.rejectRead = true
        model.perform { model.setRead(1, true) }.join()
        assertEquals("Fixture read refused", model.error)
        assertTrue(model.message!!.row.unread)
        assertTrue(model.rows.single().unread)
        assertFalse(model.working)
        core.rejectRead = false
        model.setRead(1, true)
        assertFalse(model.message!!.row.unread)
        assertTrue(model.visibleRows.isEmpty())
        assertEquals("body", model.message!!.textBody)
        assertEquals(listOf(1L to false, 1L to true, 1L to true), core.readCalls)
    }
    @Test fun pendingReadDoesNotOverwriteNewSelectionOrAcceptDuplicateAction() = runTest(dispatcher) {
        val core = FakeCore().apply { mail = listOf(row(1), row(2, unread = true)) }
        val model = model(core); model.start(); model.select(1)
        val gate = CompletableDeferred<Unit>(); core.readGate = gate
        val read = async { model.setRead(1, false) }; runCurrent()
        assertTrue(model.working)
        model.setRead(1, false)
        model.act(MailAction.Delete, 1)
        assertEquals(listOf(1L to false), core.readCalls)
        assertTrue(core.deleted.isEmpty())
        core.readGate = null
        model.select(2)
        assertFalse(model.message!!.row.unread)
        gate.complete(Unit); read.await()
        assertEquals(2L, model.message!!.row.id)
        assertFalse(model.message!!.row.unread)
        assertTrue(model.rows.first { it.id == 1L }.unread)
        assertFalse(model.working)
    }
    @Test fun pendingDeleteAllowsAnotherMessageToBeMarkedReadAndRetainsSelectionOnFailure() = runTest(dispatcher) {
        val core = FakeCore().apply { mail = listOf(row(1), row(2, unread = true)) }
        val model = model(core); model.start(); model.select(1)
        core.rejectAction = true
        model.perform { model.act(MailAction.Delete, 1) }.join()
        assertEquals(1L, model.selectedId)
        assertEquals(2, model.rows.size)
        core.rejectAction = false
        val gate = CompletableDeferred<Unit>(); core.actionGate = gate
        val action = async { model.act(MailAction.Delete, 1) }; runCurrent()
        model.setRead(1, false)
        assertTrue(core.readCalls.isEmpty())
        model.select(2)
        assertFalse(model.message!!.row.unread)
        gate.complete(Unit); action.await()
        assertEquals(2L, model.selectedId)
        assertEquals(listOf(2L), model.rows.map { it.id })
        assertEquals(listOf(1L), core.deleted)
        assertFalse(model.working)
    }
    @Test fun delayedReadRefreshCannotReplaceNewMessage() = runTest(dispatcher) {
        val core = FakeCore().apply { mail = listOf(row(1), row(2)) }
        val model = model(core); model.start(); model.select(1)
        val gate = CompletableDeferred<Unit>(); core.messageGate = gate
        val started = CompletableDeferred<Unit>(); core.messageStarted = started
        val read = async { model.setRead(1, false) }; started.await()
        core.messageGate = null
        model.select(2)
        gate.complete(Unit); read.await()
        assertEquals(2L, model.message!!.row.id)
        assertFalse(model.message!!.row.unread)
        assertTrue(model.rows.first { it.id == 1L }.unread)
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
            assertEquals(edited.copy(id = sent.id, title = sent.title), sent)
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
        val model = model(core); model.setForeground(true)
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
        runCurrent(); advanceTimeBy(5000); runCurrent()
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
    override suspend fun refreshContacts(accountId: String) = Unit
    override fun suggestContacts(accountId: String, query: String) = emptyList<ContactSuggestion>()
    var attachmentGate: CompletableDeferred<Unit>? = null
    var attachmentCalls = mutableListOf<Pair<Long, String>>()
    var rejectAttachment = false
    override suspend fun attachment(messageId: Long, section: String): ByteArray {
        attachmentCalls += messageId to section
        attachmentGate?.await()
        if (rejectAttachment) throw MailException.Failed("download failed")
        return byteArrayOf(0, -1)
    }
    override suspend fun inlineResource(id: Long, request: String): InlineResource = error("unused")
    override suspend fun loadInline(id: Long): Unit = error("unused")
    val searchCalls = mutableListOf<Triple<String?, String, UInt>>()
    var searchGate: CompletableDeferred<Unit>? = null
    var rejectSearch = false
    override suspend fun search(accountId: String?, query: String, limit: UInt, offset: UInt): List<MailRow> {
        searchCalls += Triple(accountId, query, offset)
        val snapshot = mail.filter { (accountId == null || accountId == it.account) && it.subject.contains(query) }
            .drop(offset.toInt()).take(limit.toInt())
        searchGate?.await()
        check(!rejectSearch) { "Fixture search refused" }
        return snapshot
    }
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

    override fun laterChoices() = listOf(LaterChoice("Later: this evening 18:00", 18_000),
        LaterChoice("Later: tomorrow 08:00", 86_400), LaterChoice("Later: next week, Monday 08:00", 604_800))
    val queued = mutableMapOf<String, LocalDraft>()
    override suspend fun undoSend(id: String): Boolean {
        val restored = queued.remove(id) ?: return false
        saved[id] = restored
        return true
    }
    val saved = mutableMapOf<String, LocalDraft>()
    val replyIds = mutableMapOf<String, Long?>()
    var saveCalls = 0
    var saveGate: CompletableDeferred<Unit>? = null
    var rejectSave = false
    var rejectDelete = false
    var rejectDraftList = false
    var draftListGate: CompletableDeferred<Unit>? = null
    override suspend fun localDrafts(): List<DraftRow> {
        val rows = saved.values.map { it.row() }
        draftListGate?.await()
        check(!rejectDraftList) { "draft list failed" }
        return rows
    }
    var onRestore: (() -> Unit)? = null
    override suspend fun localDraft(id: String): LocalDraft {
        onRestore?.invoke()
        return saved[id] ?: throw MailException.NotFound()
    }
    override suspend fun savedReply(id: Long): LocalDraft? = saved.values.firstOrNull {
        it.account == mail.first { row -> row.id == id }.account && it.inReplyTo == "<${id}>"
    }
    override suspend fun saveDraft(id: String, accountId: String, to: String, cc: String, subject: String, body: String,
        replyTo: Long?, attachments: List<SendAttachment>): DraftRow {
        saveCalls++
        saveGate?.await()
        check(!rejectSave) { "Fixture save refused" }
        replyIds[id] = replyTo ?: replyIds[id]
        val value = LocalDraft(id, accountId, to, cc, subject, body, replyIds[id]?.let { "<${it}>" }, emptyList(), attachments)
        saved[id] = value
        return value.row()
    }
    override suspend fun deleteDraft(id: String) { check(!rejectDelete) { "Fixture discard refused" }; saved.remove(id) }
    override suspend fun sendDraft(id: String): String {
        val draft = saved.getValue(id)
        val result = send(draft.account, listOf(draft.to), listOf(draft.cc), draft.subject, draft.body, replyIds[id], draft.attachments)
        queued[id] = draft
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
    var readGate: CompletableDeferred<Unit>? = null
    val readCalls = mutableListOf<Pair<Long, Boolean>>()
    var actionGate: CompletableDeferred<Unit>? = null
    val deleted = mutableListOf<Long>()
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
    override fun cancelBackgroundSync() {}
    override suspend fun backgroundSync(): List<MailRow> { syncAll(); return emptyList() }
    var syncs = 0
    override suspend fun syncAll() { syncs++; syncGate?.await(); check(!rejectSync) { "Fixture sync refused" } }
    override suspend fun sync(accountId: String) = syncAll()
    var conversationIds = emptyList<Long>()
    override suspend fun conversation(messageId: Long) = (conversationIds.ifEmpty { listOf(messageId) }).map { message(it) }
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
        readCalls += id to read
        readGate?.await()
        check(!rejectRead) { "Fixture read refused" }
        mail = mail.map { if (it.id == id) it.copy(unread = !read) else it }
    }
    override suspend fun send(accountId: String, to: List<String>, cc: List<String>, subject: String, bodyMarkdown: String, replyTo: Long?, attachments: List<SendAttachment>): String {
        check(!rejectSend) { "Fixture enqueue refused" }
        sent += ComposeDraft(accountId, to.joinToString(","), cc.joinToString(","), subject, bodyMarkdown, replyTo, attachments = attachments)
        return "queued"
    }
    override suspend fun later(ids: List<Long>, untilUnix: Long) {
        laterThread = Thread.currentThread()
        check(!rejectAction); snoozed = ids to untilUnix
    }
    override suspend fun done(ids: List<Long>) { check(!rejectAction); mail = mail.filterNot { it.id in ids } }
    override suspend fun delete(ids: List<Long>) {
        actionGate?.await()
        check(!rejectAction)
        deleted += ids
        mail = mail.filterNot { it.id in ids }
    }
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

private fun LocalDraft.row() = DraftRow(id, account, to, subject, 42)
