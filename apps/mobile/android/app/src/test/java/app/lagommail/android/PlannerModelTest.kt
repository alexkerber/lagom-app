package app.lagommail.android

import app.lagommail.core.*
import kotlinx.coroutines.Dispatchers
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
