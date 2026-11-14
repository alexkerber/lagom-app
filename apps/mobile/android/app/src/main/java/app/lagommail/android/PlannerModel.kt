package app.lagommail.android

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import app.lagommail.core.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import java.time.LocalDate
import java.time.ZoneId
import java.time.temporal.ChronoUnit
import java.util.Date
import java.util.Locale

/** The four top-level pages, as on desktop and iOS. The name is what the app restores at the next start. */
internal enum class AppSection(val title: String) { TODAY("Today"), MAIL("Mail"), CALENDAR("Calendar"), TODO("Todo") }

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
    var error by mutableStateOf<String?>(null)

    val unreadCount: Int get() = today?.unreadCount?.toInt() ?: 0
    val defaultList: Long? get() = (lists.firstOrNull { it.isDefault } ?: lists.firstOrNull())?.id

    fun perform(action: suspend () -> Unit) = viewModelScope.launch {
        try { action() } catch (e: CancellationException) { throw e }
        catch (e: Exception) { error = e.mailMessage() }
    }

    suspend fun reload() { reloadMail(); reloadTodos(); reloadAgenda() }

    /** Today's summary; cheap, so mail changes call it. */
    suspend fun reloadMail() {
        val core = core ?: return
        guard { today = core.today(3u) }
    }

    suspend fun reloadTodos() {
        val core = core ?: return
        guard {
            val lists = core.todoLists()
            val todos = lists.associate { it.id to core.todos(it.id, true) }
            this.lists = lists
            this.todos = todos
            today = core.today(3u)
        }
    }

    suspend fun reloadAgenda() {
        val core = core ?: return
        guard {
            val agenda = core.agenda(LocalDate.now().toString(), AGENDA_DAYS)
            calendarConnected = core.calendarConnected()
            this.agenda = agenda
        }
    }

    /** Returns false, without asking core, for a blank title. */
    suspend fun add(title: String, list: Long, due: LocalDate?): Boolean {
        val core = core ?: return false
        val trimmed = title.trim()
        if (trimmed.isEmpty()) return false
        return guard { core.addTodo(list, trimmed, due?.toString()); reloadTodos() }
    }

    suspend fun toggle(todo: Todo) {
        val core = core ?: return
        guard { core.setTodoDone(todo.id, !todo.done) }
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
    }
}
