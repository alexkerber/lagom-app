package app.lagommail.android

import app.lagommail.core.*
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.*
import org.junit.After
import org.junit.Assert.*
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.time.LocalDate
import java.time.ZoneOffset

@OptIn(ExperimentalCoroutinesApi::class)
class PlannerModelTest {
    @get:Rule val temporary = TemporaryFolder()
    private val dispatcher = StandardTestDispatcher()
    @Before fun setup() { Dispatchers.setMain(dispatcher) }
    @After fun teardown() { Dispatchers.resetMain() }
    private fun planner(core: FakeCore) = PlannerModel().apply { this.core = core }

    @Test fun addTrimsTitleSendsCoreDayAndRefusesBlankWithoutCore() = runTest(dispatcher) {
        val core = FakeCore(); val planner = planner(core); planner.reload()
        assertEquals(1L, planner.defaultList)
        assertFalse(planner.add("   ", 1, null))
        assertTrue(core.addedTodos.isEmpty())
        assertTrue(planner.add("  Pay rent ", 1, LocalDate.of(2026, 2, 3)))
        assertEquals(Triple(1L, "Pay rent", "2026-02-03"), core.addedTodos.single())
        assertEquals(listOf("Pay rent"), planner.todos[1]?.map { it.title })
        assertEquals("Today's tasks follow the change", listOf("Pay rent"), planner.today?.tasks?.map { it.title })
        assertTrue(planner.add("No date", 1, null))
        assertNull(core.addedTodos.last().third)
    }

    @Test fun toggleAndDeleteReloadEveryListWithCompletedTasks() = runTest(dispatcher) {
        val core = FakeCore(); val planner = planner(core); planner.reload()
        planner.addList("Home")
        assertEquals(listOf("Tasks", "Home"), planner.lists.map { it.name })
        planner.add("Milk", 2, null); planner.add("Bread", 2, null)
        val milk = planner.todos.getValue(2).first { it.title == "Milk" }
        planner.toggle(milk)
        assertEquals("completed tasks stay in the list, after open ones", listOf("Bread" to false, "Milk" to true),
            planner.todos.getValue(2).map { it.title to it.done })
        planner.toggle(planner.todos.getValue(2).last())
        assertFalse(planner.todos.getValue(2).first { it.title == "Milk" }.done)
        planner.delete(planner.todos.getValue(2).first { it.title == "Bread" })
        assertEquals(listOf("Milk"), planner.todos.getValue(2).map { it.title })
        assertTrue(planner.todos.getValue(1).isEmpty())
    }

    @Test fun anOlderReloadThatEndsLastNeverUndoesAChange() = runTest(dispatcher) {
        val core = FakeCore(); val planner = planner(core)
        planner.add("Milk", 1, null)
        val gate = CompletableDeferred<Unit>(); core.todosGate = gate
        val old = async { planner.reloadTodos() }; runCurrent()
        planner.toggle(planner.todos.getValue(1).single())
        assertTrue(planner.todos.getValue(1).single().done)
        gate.complete(Unit); old.await()
        assertTrue("the older snapshot must not land", planner.todos.getValue(1).single().done)
    }

    @Test fun blankListNameAddsNothing() = runTest(dispatcher) {
        val core = FakeCore(); val planner = planner(core)
        planner.addList("  "); planner.reload()
        assertEquals(1, planner.lists.size)
    }

    @Test fun refusedCallShowsItsMessageAndKeepsThePage() = runTest(dispatcher) {
        val core = FakeCore(); val planner = planner(core)
        planner.add("Keep", 1, null)
        core.rejectPlanner = true
        assertFalse(planner.add("Refused", 1, null))
        assertEquals("Fixture task refused", planner.error)
        planner.error = null
        planner.toggle(planner.todos.getValue(1).single())
        assertEquals("Fixture task refused", planner.error)
        assertEquals(listOf("Keep" to false), planner.todos.getValue(1).map { it.title to it.done })
    }

    @Test fun defaultListFallsBackToTheFirst() = runTest(dispatcher) {
        val core = FakeCore().apply { todoLists[0] = todoLists[0].copy(isDefault = false); todoLists += TodoList(5, "Work", false) }
        val planner = planner(core); planner.reloadTodos()
        assertEquals(1L, planner.defaultList)
        core.todoLists[1] = core.todoLists[1].copy(isDefault = true); planner.reloadTodos()
        assertEquals(5L, planner.defaultList)
    }

    @Test fun agendaAsksForTwoWeeksFromTodayAndTodayCountsUnreadInbox() = runTest(dispatcher) {
        val event = AgendaItem("2026-11-14", "fixture", "Personal", "Plan", 10, 20, false, null, null)
        val core = FakeCore().apply { events = listOf(event); connected = true; mail = listOf(row(1, unread = true), row(2), row(3, unread = true)) }
        val planner = planner(core); planner.reload()
        assertEquals(LocalDate.now().toString() to 14u, core.agendaCalls.single())
        assertEquals(listOf(event), planner.agenda); assertTrue(planner.calendarConnected)
        assertEquals(2, planner.unreadCount)
        assertEquals(listOf(1L, 3L), planner.today?.unread?.map { it.id })
    }

    @Test fun weekStartsOnMondayAndMovesByWeeks() = runTest(dispatcher) {
        val core = FakeCore(); val planner = planner(core)
        assertEquals("Sunday belongs to the week before", LocalDate.of(2026, 11, 9), PlannerModel.monday(LocalDate.of(2026, 11, 15)))
        assertEquals(LocalDate.of(2026, 11, 9), PlannerModel.monday(LocalDate.of(2026, 11, 9)))
        val thisWeek = PlannerModel.monday(LocalDate.now())
        planner.showWeek(1)
        assertEquals(thisWeek.plusWeeks(1), planner.weekStart)
        assertEquals(thisWeek.plusWeeks(1).toString() to 7u, core.agendaCalls.last())
        planner.showWeek(-2); planner.showWeek(0)
        assertEquals(thisWeek, planner.weekStart)
    }

    @Test fun weekBlocksShareOverlapsInLanesAsOnDesktop() {
        val day = LocalDate.of(2026, 11, 10)
        val midnight = day.atStartOfDay(ZoneOffset.UTC).toEpochSecond()
        fun event(from: Double, to: Double, allDay: Boolean = false) =
            AgendaItem("2026-11-10", "a", "c", "", midnight + (from * 3600).toLong(), midnight + (to * 3600).toLong(), allDay, null, null)
        val events = listOf(event(9.0, 10.0), event(9.5, 11.0), event(10.0, 10.25), event(12.0, 12.1), event(-2.0, 1.0), event(23.0, 26.0),
            event(0.0, 24.0, allDay = true), event(23.98, 24.0))
        val blocks = weekBlocks(events, day, ZoneOffset.UTC)
        assertEquals("all-day events stay off the grid", listOf(4, 0, 1, 2, 3, 5, 7), blocks.map { it.index })
        assertEquals("the 10:00 event reuses the lane the 9:00 one left", listOf(0, 0, 1, 0, 0, 0, 1), blocks.map { it.lane })
        assertEquals(listOf(1, 2, 2, 2, 1, 2, 2), blocks.map { it.lanes })
        assertEquals("a 23:59 event moves up to stay 36 minutes tall", 36.0, blocks[6].end - blocks[6].start, 1e-9)
        assertEquals("yesterday's overnight event starts at the top", 0.0, blocks[0].start, 0.0)
        assertEquals("a six-minute event draws 36 minutes tall", 12 * 60 + 36.0, blocks[4].end, 1e-9)
        assertEquals("an event past midnight ends at the bottom", 1440.0, blocks[5].end, 0.0)
    }

    @Test fun withoutCoreNothingLoadsOrFails() = runTest(dispatcher) {
        val planner = PlannerModel()
        planner.reload()
        assertFalse(planner.add("Task", 1, null))
        assertNull(planner.today); assertNull(planner.error); assertEquals(0, planner.unreadCount)
    }

    @Test fun dueChipsAndDayTitlesUseRelativeWordsNearToday() {
        val words = DateWords(LocalDate.of(2026, 11, 14)) { date, skeleton -> "$skeleton $date" }
        assertEquals("Yesterday" to true, words.due("2026-11-13"))
        assertEquals("Today" to false, words.due("2026-11-14"))
        assertEquals("Tomorrow" to false, words.due("2026-11-15T09:00:00Z"))
        assertEquals("EEEE 2026-11-20" to false, words.due("2026-11-20"))
        assertEquals("dMMM 2026-11-21" to false, words.due("2026-11-21"))
        assertEquals("dMMM 2026-11-01" to true, words.due("2026-11-01"))
        assertEquals("dMMMyyyy 2027-01-02" to false, words.due("2027-01-02"))
        assertNull(words.due("soon")); assertNull(words.due("2026-02-30"))
        assertEquals("Today", words.dayTitle("2026-11-14"))
        assertEquals("Tomorrow", words.dayTitle("2026-11-15"))
        assertEquals("EEEEdMMMM 2026-11-13", words.dayTitle("2026-11-13"))
        assertEquals("not a day", words.dayTitle("not a day"))
    }
}

@OptIn(ExperimentalCoroutinesApi::class)
class OpenInboxTest {
    @get:Rule val temporary = TemporaryFolder()
    private val dispatcher = StandardTestDispatcher()
    @Before fun setup() { Dispatchers.setMain(dispatcher) }
    @After fun teardown() { Dispatchers.resetMain() }

    @Test fun todayMailOpensTheUnifiedInboxAndTheMessage() = runTest(dispatcher) {
        val core = FakeCore().apply { mail = listOf(row(7, unread = true)) }
        val model = MailModel(open = { core }, attachmentDirectory = File(temporary.newFolder(), "attachments"))
        model.start()
        model.changeScope("fixture", Mailbox.SENT)
        model.changeSearch("old search")
        model.openInbox(7)
        assertNull(model.accountId); assertEquals(Mailbox.INBOX, model.mailbox); assertEquals("", model.searchText)
        assertEquals(7L, model.selectedId); assertEquals(7L, model.message?.row?.id)
        model.clearSelection()
        model.openInbox(null)
        assertNull(model.selectedId); assertEquals(listOf(7L), model.rows.map { it.id })
    }
}
