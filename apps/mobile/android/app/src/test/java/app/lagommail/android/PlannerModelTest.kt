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
        assertEquals("the heading counts only open tasks", 1, planner.openCount(2))
        assertEquals(0, planner.openCount(1))
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
        val google = MailAccount("google", "fixture@gmail.com", "Fixture")
        val core = FakeCore().apply { events = listOf(event); connected = true; calendarAccess = listOf(google); mail = listOf(row(1, unread = true), row(2), row(3, unread = true)) }
        val planner = planner(core); planner.reload()
        assertEquals("Calendar offers access for this account", listOf(google), planner.calendarAccess)
        assertEquals(LocalDate.now().toString() to 14u, core.agendaCalls.first())
        assertEquals("the tablet week loads with it", PlannerModel.monday(LocalDate.now()).toString() to 7u, core.agendaCalls[1])
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

    @Test fun newWeekSelectsTodayWhenContainedOtherwiseItsFirstDay() {
        val start = LocalDate.of(2026, 11, 16)
        for (offset in 0L..6L) {
            val today = start.plusDays(offset)
            assertEquals(today, calendarDay(start, today))
        }
        assertEquals(start, calendarDay(start, start.minusDays(1)))
        assertEquals(start, calendarDay(start, start.plusDays(7)))
        val yearEnd = LocalDate.of(2026, 12, 28)
        assertEquals(LocalDate.of(2027, 1, 1), calendarDay(yearEnd, LocalDate.of(2027, 1, 1)))
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

    @Test fun monthTitleShowsTheYearOnlyOutsideThisYear() {
        val words = DateWords(LocalDate.of(2026, 11, 20)) { date, skeleton -> "$skeleton $date" }
        assertEquals("MMMM 2026-11-02" to null, words.monthTitle(LocalDate.of(2026, 11, 2)))
        assertEquals("MMMM 2027-01-04" to "yyyy 2027-01-04", words.monthTitle(LocalDate.of(2027, 1, 4)))
        assertEquals("MMMM 2025-12-29" to "yyyy 2025-12-29", words.monthTitle(LocalDate.of(2025, 12, 29)))
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

    @Test fun newEventDefaultsAsTheMac() {
        val day = LocalDate.of(2026, 11, 20)
        val at = { h: Int, m: Int -> day.atTime(h, m) }
        assertEquals(at(10, 15) to at(11, 15), newEventTimes(day, at(10, 1)))
        assertEquals("a quarter hour stays", at(10, 15) to at(11, 15), newEventTimes(day, at(10, 15)))
        assertEquals("never past 23:45, and the hour runs into tomorrow", at(23, 45) to day.plusDays(1).atTime(0, 45), newEventTimes(day, at(23, 50)))
        assertEquals("another day opens at 09:00", day.plusDays(1).atTime(9, 0) to day.plusDays(1).atTime(10, 0), newEventTimes(day.plusDays(1), at(16, 0)))
    }

    @Test fun eventFormSendsInstantsAndDaysAndCreateReportsCoreWords() = runTest(dispatcher) {
        val zone = java.time.ZoneId.of("Europe/Stockholm")
        val start = LocalDate.of(2026, 11, 20).atTime(9, 0)
        val form = eventForm("Lunch", start, start.plusDays(2), allDay = false, guests = "a@b.example", description = "", meet = true, zone = zone)
        assertEquals(start.atZone(zone).toEpochSecond(), form.startUnix)
        assertEquals(form.startUnix + 2 * 86_400, form.endUnix)
        assertEquals(Triple("2026-11-20", "2026-11-22", "Europe/Stockholm"), Triple(form.firstDay, form.lastDay, form.timeZone))

        val core = FakeCore().apply { connected = true; calendars = listOf(EventCalendar("acct", "primary", "Personal · you@gmail.com")) }
        val planner = PlannerModel().apply { this.core = core }; planner.reload()
        assertEquals(core.calendars, planner.eventCalendars)
        core.rejectEvent = "End must be later than start"
        assertEquals("End must be later than start", planner.createEvent(planner.eventCalendars[0], form))
        core.rejectEvent = null
        val loads = core.agendaCalls.size
        assertNull(planner.createEvent(planner.eventCalendars[0], form))
        assertEquals("acct" to "primary", core.createdEvents.single().let { it.first to it.second })
        assertTrue("the week reloads after saving", core.agendaCalls.size > loads)

        val demo = PlannerModel().apply { this.core = core; this.demo = true }; demo.reload()
        assertEquals(listOf("Demo calendar"), demo.eventCalendars.map { it.label })
        assertNull(demo.createEvent(demo.eventCalendars[0], form))
        assertEquals(1, core.demoEvents.size); assertEquals(1, core.createdEvents.size)
    }
}
