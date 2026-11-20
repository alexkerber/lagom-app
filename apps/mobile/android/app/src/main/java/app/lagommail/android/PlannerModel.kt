package app.lagommail.android

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import app.lagommail.core.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import java.time.Instant
import java.time.LocalDate
import java.time.LocalDateTime
import java.time.ZoneId
import java.time.temporal.ChronoUnit
import java.util.Date
import java.util.Locale

/** The four top-level pages, as on desktop and iOS. The name is what the app restores at the next start. */
internal enum class AppSection(val title: String) { TODAY("Today"), MAIL("Mail"), CALENDAR("Calendar"), TODO("To-do") }

/** A localized date from an ICU skeleton such as "EEEEdMMMM", in the order and words of the device's language. */
internal fun localDate(date: LocalDate, skeleton: String): String =
    android.icu.text.DateFormat.getInstanceForSkeleton(skeleton, Locale.getDefault())
        .format(Date.from(date.atStartOfDay(ZoneId.systemDefault()).toInstant()))

/** Due chips and agenda titles, after iOS `DateWords` and the desktop's task chips. `format` takes an ICU skeleton. */
internal class DateWords(
    private val today: LocalDate = LocalDate.now(),
    private val format: (LocalDate, String) -> String = ::localDate,
) {
    /** A core day (`YYYY-MM-DD`, or the date part of an RFC 3339 instant). */
    fun day(value: String): LocalDate? {
        val parts = value.take(10).split("-").mapNotNull { it.toIntOrNull() }
        if (parts.size != 3) return null
        return runCatching { LocalDate.of(parts[0], parts[1], parts[2]) }.getOrNull()
    }

    /** A due chip: relative words near today, then a date; `overdue` before today. */
    fun due(value: String): Pair<String, Boolean>? {
        val date = day(value) ?: return null
        val offset = ChronoUnit.DAYS.between(today, date)
        val label = when (offset) {
            -1L -> "Yesterday"
            0L -> "Today"
            1L -> "Tomorrow"
            in 2L..6L -> format(date, "EEEE")
            else -> format(date, if (date.year == today.year) "dMMM" else "dMMMyyyy")
        }
        return label to (offset < 0)
    }

    /** An agenda section title. */
    fun dayTitle(value: String): String {
        val date = day(value) ?: return value
        return when (ChronoUnit.DAYS.between(today, date)) {
            0L -> "Today"
            1L -> "Tomorrow"
            else -> format(date, "EEEEdMMMM")
        }
    }

    /** The calendar header, desktop's month and quiet year: the year only outside this year, so a phone header stays one line. */
    fun monthTitle(date: LocalDate): Pair<String, String?> =
        format(date, "MMMM") to (if (date.year == today.year) null else format(date, "yyyy"))
}

/** Today, Todo and Calendar, as iOS `PlannerModel`. Core owns the task and event rules; this holds what the pages show. */
internal class PlannerModel : ViewModel() {
    var core: LagomCoreInterface? = null
    var today by mutableStateOf<TodaySummary?>(null); private set
    var lists by mutableStateOf<List<TodoList>>(emptyList()); private set
    /** Each list's tasks, open first, as core orders them. */
    var todos by mutableStateOf<Map<Long, List<Todo>>>(emptyMap()); private set
    var agenda by mutableStateOf<List<AgendaItem>>(emptyList()); private set
    var calendarConnected by mutableStateOf(false); private set
    /** Google accounts that still need calendar access: Calendar and Today offer it. */
    var calendarAccess by mutableStateOf<List<MailAccount>>(emptyList()); private set
    /** A Google account allows calendar access; until its first fetch Calendar waits for the sync. */
    var calendarGranted by mutableStateOf(false); private set
    /** Google accounts with Calendar but without Google Tasks: To-do offers Tasks again. */
    var tasksAccess by mutableStateOf<List<MailAccount>>(emptyList()); private set
    /** Calendar's week: its Monday and that week's events. */
    // shortcut: a time zone change keeps the old Monday until Today is tapped, as on iOS.
    var weekStart by mutableStateOf(monday(LocalDate.now())); private set
    var week by mutableStateOf<List<AgendaItem>>(emptyList()); private set
    var error by mutableStateOf<String?>(null)
    /** Calendars New event can write to; empty hides New event. A demo store gets the Mac's demo calendar. */
    var eventCalendars by mutableStateOf<List<EventCalendar>>(emptyList()); private set
    /** A demo store: New event goes into its fixture calendar, never to Google. */
    var demo = false

    val unreadCount: Int get() = today?.unreadCount?.toInt() ?: 0
    val defaultList: Long? get() = (lists.firstOrNull { it.isDefault } ?: lists.firstOrNull())?.id
    /** A list's tasks still to do, for its heading. */
    fun openCount(list: Long): Int = todos[list].orEmpty().count { !it.done }

    fun perform(action: suspend () -> Unit) = viewModelScope.launch {
        try { action() } catch (e: CancellationException) { throw e }
        catch (e: Exception) { error = e.mailMessage() }
    }

    suspend fun reload() { reloadMail(); reloadTodos(); reloadAgenda() }

    // Loads overlap (start, pull, a change); only the newest request of each kind lands.
    private var todayRevision = 0
    private var todosRevision = 0
    private var agendaRevision = 0
    private var weekRevision = 0

    /** Today's summary; cheap, so mail changes call it. */
    suspend fun reloadMail() {
        val core = core ?: return
        val request = ++todayRevision
        guard { core.today(3u).let { if (request == todayRevision) today = it } }
    }

    suspend fun reloadTodos() {
        val core = core ?: return
        val request = ++todosRevision
        guard {
            val lists = core.todoLists()
            val todos = lists.associate { it.id to core.todos(it.id, true) }
            val access = core.tasksAccessNeeded()
            if (request != todosRevision) return@guard
            this.lists = lists
            this.todos = todos
            tasksAccess = access
        }
        reloadMail()
    }

    suspend fun reloadAgenda() {
        val core = core ?: return
        val request = ++agendaRevision
        guard {
            val agenda = core.agenda(LocalDate.now().toString(), AGENDA_DAYS)
            val connected = core.calendarConnected()
            val access = core.calendarAccessNeeded()
            val granted = core.calendarGranted()
            // The demo calendar only in the store that has one, not in the demo's empty states.
            val calendars = if (demo) (if (connected) listOf(EventCalendar("", "demo", "Demo calendar")) else emptyList()) else core.eventCalendars()
            if (request != agendaRevision) return@guard
            eventCalendars = calendars
            calendarConnected = connected
            calendarAccess = access
            calendarGranted = granted
            this.agenda = agenda
        }
        // As iOS: the start's reload also fills the week, which may have asked before the store opened.
        reloadWeek()
    }

    suspend fun reloadWeek() {
        val core = core ?: return
        val request = ++weekRevision
        guard { core.agenda(weekStart.toString(), 7u).let { if (request == weekRevision) week = it } }
    }

    /** Moves the week by `weeks`; 0 goes back to this week. */
    suspend fun showWeek(weeks: Long) {
        weekStart = if (weeks == 0L) monday(LocalDate.now()) else weekStart.plusWeeks(weeks)
        week = emptyList()
        reloadWeek()
    }

    /** Desktop `save_new_event`: null once Google (or the demo store) has the event and the week shows it, else core's words. */
    suspend fun createEvent(calendar: EventCalendar, form: EventForm): String? {
        val core = core ?: return "lagom is not ready yet."
        try {
            if (demo) core.addDemoEvent(form) else core.createEvent(calendar.account, calendar.calendar, form)
        } catch (e: CancellationException) { throw e } catch (e: Exception) { return e.mailMessage() }
        reloadAgenda()
        return null
    }

    /** New event's save, owned here and not by the dialog: a dialog restored after rotation must neither send it
     *  again nor miss its answer. */
    var savingEvent by mutableStateOf(false); private set
    /** The save's answer for the dialog: "" once saved, else core's words; null while none waits. */
    var eventAnswer by mutableStateOf<String?>(null)

    fun saveEvent(calendar: EventCalendar, form: EventForm) {
        if (savingEvent) return
        savingEvent = true
        eventAnswer = null
        viewModelScope.launch { try { eventAnswer = createEvent(calendar, form) ?: "" } finally { savingEvent = false } }
    }

    /** Returns false, without asking core, for a blank title. */
    suspend fun add(title: String, list: Long, due: LocalDate?): Boolean {
        val core = core ?: return false
        val trimmed = title.trim()
        if (trimmed.isEmpty()) return false
        return try { core.addTodo(list, trimmed, due?.toString()); reloadTodos(); true }
        catch (e: CancellationException) { throw e }
        // Named, since the field may already hold the next task.
        catch (e: Exception) { error = "Could not add “$trimmed”: ${e.mailMessage()}"; false }
    }

    /** Saves at once; [settleMillis] keeps the lists as drawn a moment longer, so a tick is seen before its row moves. */
    suspend fun toggle(todo: Todo, settleMillis: Long = 0) {
        val core = core ?: return
        guard { core.setTodoDone(todo.id, !todo.done) }
        delay(settleMillis)
        reloadTodos()
    }

    suspend fun delete(todo: Todo) {
        val core = core ?: return
        guard { core.deleteTodo(todo.id) }
        reloadTodos()
    }

    suspend fun addList(name: String) {
        val core = core ?: return
        val trimmed = name.trim()
        if (trimmed.isEmpty()) return
        guard { core.addTodoList(trimmed) }
        reloadTodos()
    }

    /** As iOS: a failed call shows its message and leaves the page as it was. */
    private suspend fun guard(block: suspend () -> Unit): Boolean = try { block(); true }
        catch (e: CancellationException) { throw e }
        catch (e: Exception) { error = e.mailMessage(); false }

    companion object {
        /** How far ahead Calendar lists cached events. */
        const val AGENDA_DAYS = 14u
        /** The week's Monday; Sunday belongs to the week before. */
        fun monday(day: LocalDate): LocalDate = day.minusDays(day.dayOfWeek.value - 1L)
    }
}

/** A new week opens on today when it contains today, otherwise on its first day. */
internal fun calendarDay(weekStart: LocalDate, today: LocalDate): LocalDate =
    if (today >= weekStart && today < weekStart.plusDays(7)) today else weekStart

/** Desktop `new_event_default`: on today the next quarter hour (at most 23:45), on another day 09:00; an hour long. */
internal fun newEventTimes(day: LocalDate, now: LocalDateTime): Pair<LocalDateTime, LocalDateTime> {
    val minute = if (day == now.toLocalDate()) minOf((now.hour * 60 + now.minute + 14) / 15 * 15, 1425) else 9 * 60
    val start = day.atStartOfDay().plusMinutes(minute.toLong())
    return start to start.plusMinutes(60)
}

/** The core's form from the pickers: instants for a timed event, the first and last day for an all-day one. */
internal fun eventForm(title: String, start: LocalDateTime, end: LocalDateTime, allDay: Boolean, guests: String, description: String,
    meet: Boolean, zone: ZoneId = ZoneId.systemDefault()): EventForm =
    EventForm(title, allDay, start.atZone(zone).toEpochSecond(), end.atZone(zone).toEpochSecond(), start.toLocalDate().toString(),
        end.toLocalDate().toString(), zone.id, guests, description, meet)

/** Desktop `interval`: a wall-clock time that daylight saving skips or repeats has no single instant. */
internal fun unambiguous(at: LocalDateTime, zone: ZoneId = ZoneId.systemDefault()) = zone.rules.getValidOffsets(at).size == 1

/** A timed event placed on the week grid, desktop `week::layout`: minutes from midnight, and its lane among overlaps. */
internal data class WeekBlock(val index: Int, val start: Double, val end: Double, val lane: Int = 0, val lanes: Int = 1)

/** iOS `weekBlocks`: timed events on `day` as blocks at least 36 minutes tall; overlapping events share the width in lanes. */
internal fun weekBlocks(events: List<AgendaItem>, day: LocalDate, zone: ZoneId = ZoneId.systemDefault()): List<WeekBlock> {
    val from = day.atStartOfDay(zone).toEpochSecond()
    val to = day.plusDays(1).atStartOfDay(zone).toEpochSecond()
    fun minute(unix: Long): Double = Instant.ofEpochSecond(unix).atZone(zone).toLocalTime().let { it.hour * 60 + it.minute + it.second / 60.0 }
    // shortcut: a repeated DST hour shares one wall-clock row, as on desktop.
    val blocks = events.withIndex().filter { (_, e) -> !e.allDay && e.startUnix < to && e.endUnix > from }.map { (i, e) ->
        val top = if (e.startUnix <= from) 0.0 else minute(e.startUnix)
        val bottom = if (e.endUnix >= to) 1440.0 else minute(e.endUnix)
        // A late event keeps its 36 minutes by starting earlier, so it never draws flat at the bottom.
        val start = minOf(top, 1440.0 - 36)
        WeekBlock(i, start, minOf(maxOf(bottom, start + 36), 1440.0))
    }.sortedWith(compareBy({ it.start }, { it.end }, { it.index })).toMutableList()
    var group = 0
    var until = 0.0
    val lanes = mutableListOf<Double>()
    for (i in blocks.indices) {
        if (blocks[i].start >= until) {
            for (j in group until i) blocks[j] = blocks[j].copy(lanes = lanes.size)
            group = i
            lanes.clear()
        }
        val free = lanes.indexOfFirst { it <= blocks[i].start }
        val lane = if (free < 0) lanes.size else free
        if (lane == lanes.size) lanes += blocks[i].end else lanes[lane] = blocks[i].end
        blocks[i] = blocks[i].copy(lane = lane)
        until = if (group == i) blocks[i].end else maxOf(until, blocks[i].end)
    }
    for (j in group until blocks.size) blocks[j] = blocks[j].copy(lanes = lanes.size)
    return blocks
}
