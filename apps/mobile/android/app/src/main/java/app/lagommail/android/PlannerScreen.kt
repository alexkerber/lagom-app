package app.lagommail.android

import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.Uri
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.addPathNodes
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.*
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.em
import androidx.compose.ui.unit.sp
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.withLink
import androidx.compose.ui.text.withStyle
import java.time.LocalTime
import app.lagommail.core.*
import kotlinx.coroutines.delay
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId
import java.time.ZoneOffset
import java.util.Date

/** The desktop sidebar's page icon, as iOS `AppSection.icon`. */
internal val AppSection.icon: Int get() = when (this) {
    AppSection.TODAY -> R.drawable.ic_home
    AppSection.MAIL -> R.drawable.ic_mail
    AppSection.CALENDAR -> R.drawable.ic_calendar
    AppSection.TODO -> R.drawable.ic_todo
}

/** The minute, kept current so the now line and past events move on their own. */
@Composable private fun nowSeconds(): Long {
    val now by produceState(System.currentTimeMillis() / 1000) {
        while (true) { delay(60_000 - System.currentTimeMillis() % 60_000); value = System.currentTimeMillis() / 1000 }
    }
    return now
}

@Composable private fun clock(unix: Long): String =
    android.text.format.DateFormat.getTimeFormat(LocalContext.current).format(Date(unix * 1000))

/** A page title as on desktop and iOS: semibold, actions on the right. */
@Composable private fun PageTitle(title: String, trailing: @Composable RowScope.() -> Unit = {}) {
    TopBar(Modifier.statusBarsPadding()) {
        BarTitle(title, Modifier.weight(1f))
        trailing()
    }
}

/** Schedule, To do, Mail, a task list's name. */
@Composable private fun Heading(title: String, modifier: Modifier = Modifier) =
    Text(title, modifier.padding(bottom = 8.dp).semantics { heading() }, style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.SemiBold)

/** Small Geist Mono capitals, the site's labels: Today's date and section titles. */
@Composable private fun MonoLabel(text: String, modifier: Modifier = Modifier) =
    Text(text.uppercase(), modifier.semantics { heading(); contentDescription = text }, color = MaterialTheme.colorScheme.onSurfaceVariant,
        fontFamily = LabelFace, fontWeight = FontWeight.Medium, fontSize = 13.sp)

/** The desktop's Today sentence in two tones (Alex 2026-11-17): the counts are ink and open their page. */
@Composable private fun TodaySentence(planner: PlannerModel, open: (AppSection) -> Unit, openMail: (Long?) -> Unit) {
    val wide = LocalConfiguration.current.screenWidthDp >= 600
    val ink = MaterialTheme.colorScheme.onSurface
    val greeting = when (LocalTime.now().hour) { in 0..11 -> "Good morning."; in 12..17 -> "Good afternoon."; else -> "Good evening." }
    fun count(n: Int, one: String, many: String, none: String) = when (n) { 0 -> none; 1 -> "1 $one"; else -> "$n $many" }
    val text = buildAnnotatedString {
        withStyle(SpanStyle(color = ink)) { append(greeting + "\n") }
        fun link(label: String, go: () -> Unit) = withLink(LinkAnnotation.Clickable(label, TextLinkStyles(SpanStyle(color = ink))) { go() }) { append(label) }
        link(count(planner.unreadCount, "unread", "unread", "Nothing unread")) { openMail(null) }
        append(", ")
        link(count(planner.today?.events?.size ?: 0, "event", "events", "no events")) { open(AppSection.CALENDAR) }
        append(" and ")
        // Only tasks due today or earlier, as desktop `due_today`; Today's list also shows undated and future ones.
        val due = planner.today?.let { today -> today.tasks.count { task -> task.due?.take(10)?.let { it <= today.day } ?: false } } ?: 0
        link(count(due, "task due", "tasks due", "nothing due")) { open(AppSection.TODO) }
        append(".")
    }
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        MonoLabel(localDate(LocalDate.now(), "EEEEdMMMM"))
        Text(text, Modifier.semantics { heading() }, color = MaterialTheme.colorScheme.onSurfaceVariant, fontFamily = DisplayFace,
            fontWeight = FontWeight.Bold, fontSize = if (wide) 48.sp else 34.sp, letterSpacing = DisplayTracking.em, lineHeight = if (wide) 50.sp else 36.sp)
    }
}

@Composable private fun Quiet(text: String) =
    Text(text, color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodyMedium)

/** "+ New task", "+ Add task": a quiet line that adds. */
@Composable private fun AddRow(title: String, enabled: Boolean = true, onClick: () -> Unit) {
    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).clickable(enabled = enabled, role = Role.Button, onClick = onClick).padding(horizontal = 8.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        val ink = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = if (enabled) 1f else 0.38f)
        LagomIcon(R.drawable.ic_plus, Modifier.padding(horizontal = 2.dp), tint = ink)
        Text(title, color = ink)
    }
}


/** Pull to refresh for a planner page; the indicator shows only for the pull. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable private fun Refreshable(planner: PlannerModel, refresh: suspend () -> Unit, modifier: Modifier, content: @Composable () -> Unit) {
    var pulled by remember { mutableStateOf(false) }
    PullToRefreshBox(pulled, onRefresh = { pulled = true; planner.perform { try { refresh() } finally { pulled = false } } }, modifier) { content() }
}

private fun openLink(context: android.content.Context, planner: PlannerModel, link: String) {
    try { context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(link))) }
    catch (_: ActivityNotFoundException) { planner.error = "No browser can open this page" }
}

/** Only web meeting links: an event's link comes from whoever sent the invite. */
private fun AgendaItem.webLink(): String? = joinUrl?.takeIf { it.startsWith("https://", true) || it.startsWith("http://", true) }

/** The calm Today page, as iOS `TodayView`: schedule, what is due, and a short look at unread mail. */
@Composable internal fun TodayScreen(planner: PlannerModel, model: MailModel, open: (AppSection) -> Unit, openMail: (Long?) -> Unit) {
    var newTask by rememberSaveable { mutableStateOf(false) }
    val now = nowSeconds()
    val words = DateWords()
    Column(Modifier.fillMaxSize()) {
        Refreshable(planner, { planner.reload() }, Modifier.weight(1f)) {
            Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(32.dp)) {
                TodaySentence(planner, open, openMail)
                Column {
                    MonoLabel("Schedule", Modifier.padding(bottom = 8.dp))
                    val events = planner.today?.events.orEmpty()
                    if (events.isEmpty()) Quiet(if (planner.calendarConnected) "Nothing scheduled today."
                        else "Your calendar shows here once a Google account is connected. Google Calendar sync is coming to Android.")
                    // The now line sits before the first timed event still to start, as on desktop.
                    val nowIndex = events.indexOfFirst { !it.allDay && it.startUnix > now }.let { if (it < 0) events.size else it }
                    events.forEachIndexed { index, event ->
                        if (index == nowIndex) NowLine(now)
                        EventRow(planner, event, now)
                    }
                    if (events.isNotEmpty() && nowIndex == events.size) NowLine(now)
                }
                Column {
                    MonoLabel("To do", Modifier.padding(bottom = 8.dp))
                    val tasks = planner.today?.tasks.orEmpty()
                    if (tasks.isEmpty()) Quiet("Nothing due today.")
                    tasks.forEach { todo -> TodoRow(todo, words) { planner.perform { planner.toggle(todo) } } }
                    AddRow("New task", enabled = planner.lists.isNotEmpty()) { newTask = true }
                }
                Column {
                    MonoLabel("Mail", Modifier.padding(bottom = 8.dp))
                    val unread = planner.today?.unread.orEmpty()
                    if (unread.isEmpty()) Quiet(if (model.accounts.isEmpty()) "Add an account in Mail to see new mail here." else "No unread mail in Inbox.")
                    unread.forEach { row ->
                        val view = RowPresentation(row)
                        Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).clickable(onClickLabel = "Open the message") { openMail(row.id) },
                            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                            Box(Modifier.size(7.dp).background(MaterialTheme.colorScheme.primary, CircleShape))
                            Text(view.sender, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            Text(view.subject, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        }
                    }
                }
            }
        }
    }
    if (newTask) NewTaskDialog(planner, planner.defaultList, LocalDate.now()) { newTask = false }
}

/** The current time in gold across the schedule. */
@Composable private fun NowLine(now: Long) {
    val time = clock(now)
    val gold = lagom.later
    Row(Modifier.fillMaxWidth().heightIn(min = 28.dp).clearAndSetSemantics { contentDescription = "Now, $time" },
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(time, Modifier.widthIn(min = 52.dp), color = gold, style = MaterialTheme.typography.labelSmall.copy(fontFeatureSettings = "tnum"))
        Box(Modifier.weight(1f).height(2.dp).background(gold, RoundedCornerShape(1.dp)))
    }
}

/** One schedule line, desktop `agenda_view`: time, a bar, the title, and a meeting link. */
@Composable private fun EventRow(planner: PlannerModel, event: AgendaItem, now: Long) {
    val context = LocalContext.current
    val colors = MaterialTheme.colorScheme
    val title = event.title.ifEmpty { "(No title)" }
    val past = !event.allDay && event.endUnix < now
    val start = clock(event.startUnix)
    val end = clock(maxOf(event.startUnix, event.endUnix))
    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).semantics(mergeDescendants = true) {
        contentDescription = listOfNotNull(if (event.allDay) "All day" else null, title, event.location?.takeIf { it.isNotEmpty() }).joinToString(", ")
        if (!event.allDay) stateDescription = "$start – $end"
    }, verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(if (event.allDay) "All day" else start, Modifier.widthIn(min = 52.dp).clearAndSetSemantics {},
            color = colors.onSurfaceVariant, style = MaterialTheme.typography.labelSmall.copy(fontFeatureSettings = "tnum"))
        Box(Modifier.width(2.dp).height(28.dp).background(colors.primary, RoundedCornerShape(1.dp)))
        Column(Modifier.weight(1f).clearAndSetSemantics {}) {
            Text(title, color = if (past) colors.onSurfaceVariant else colors.onSurface, maxLines = 1, overflow = TextOverflow.Ellipsis)
            event.location?.takeIf { it.isNotEmpty() }?.let {
                Text(it, color = colors.onSurfaceVariant, style = MaterialTheme.typography.bodySmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
        event.webLink()?.let { link ->
            TextButton(onClick = { openLink(context, planner, link) },
                modifier = Modifier.sizeIn(minWidth = 48.dp, minHeight = 48.dp).semantics { contentDescription = "Join meeting" }) { Text("Join") }
        }
    }
}

/** A task, desktop `task_row`: a completion circle, its title, a mail mark, and the due chip. */
@Composable private fun TodoRow(todo: Todo, words: DateWords, modifier: Modifier = Modifier, toggle: () -> Unit) {
    val colors = MaterialTheme.colorScheme
    Row(modifier.fillMaxWidth().heightIn(min = 48.dp).padding(end = 8.dp), verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        // A checkbox named after its task; iOS's words are the action TalkBack offers.
        Box(Modifier.size(48.dp).toggleable(todo.done, role = Role.Checkbox) { toggle() }.semantics {
            contentDescription = todo.title
            onClick(if (todo.done) "Mark as not done" else "Mark as done") { toggle(); true }
        }, contentAlignment = Alignment.Center) {
            Box(Modifier.size(21.dp).border(1.dp, colors.onSurfaceVariant, CircleShape), contentAlignment = Alignment.Center) {
                if (todo.done) LagomIcon(R.drawable.ic_done, size = 14.dp)
            }
        }
        Text(todo.title, Modifier.weight(1f), color = if (todo.done) colors.onSurfaceVariant else colors.onSurface,
            textDecoration = if (todo.done) TextDecoration.LineThrough else null)
        if (todo.fromMail) LagomIcon(R.drawable.ic_linked_mail, size = 16.dp, label = "From mail")
        val due = if (todo.done) null else todo.due?.let(words::due)
        if (due != null) {
            val (label, overdue) = due
            Text(label, Modifier.background(if (overdue) colors.error.copy(alpha = 0.12f) else colors.surfaceVariant, RoundedCornerShape(6.dp))
                .padding(horizontal = 7.dp, vertical = 3.dp).semantics { contentDescription = if (overdue) "Overdue, due $label" else "Due $label" },
                color = if (overdue) colors.error else colors.onSurfaceVariant, style = MaterialTheme.typography.labelSmall)
        }
    }
}

/** Local task lists, as iOS `TodoView` and the desktop's Todo page. */
@Composable internal fun TodoScreen(planner: PlannerModel) {
    var adding by rememberSaveable { mutableStateOf<Long?>(null) }
    var newList by rememberSaveable { mutableStateOf(false) }
    var menu by remember { mutableStateOf(false) }
    var showDone by remember { mutableStateOf<Set<Long>>(emptySet()) }
    val words = DateWords()
    Column(Modifier.fillMaxSize()) {
        PageTitle("Todo") {
            Box {
                LagomIconButton("Add", R.drawable.ic_plus) { menu = true }
                DropdownMenu(menu, { menu = false }) {
                    DropdownMenuItem(text = { Text("New task") }, enabled = planner.lists.isNotEmpty(), onClick = { menu = false; adding = planner.defaultList })
                    DropdownMenuItem(text = { Text("New list") }, onClick = { menu = false; newList = true })
                }
            }
        }
        Refreshable(planner, { planner.reloadTodos() }, Modifier.weight(1f)) {
            LazyColumn(Modifier.fillMaxSize().padding(horizontal = 16.dp)) {
                if (planner.lists.isEmpty()) item(key = "empty") { EmptyState(R.drawable.ic_todo, "No lists", Modifier.fillParentMaxSize(), "Add a list to start.") }
                planner.lists.forEach { list ->
                    val all = planner.todos[list.id].orEmpty()
                    val done = all.filter { it.done }
                    item(key = "list-${list.id}") { Heading(list.name, Modifier.padding(top = 16.dp, bottom = 4.dp)) }
                    items(all.filter { !it.done }, key = { it.id }) { DeletableTodoRow(planner, it, words) }
                    item(key = "add-${list.id}") { AddRow("Add task") { adding = list.id } }
                    if (done.isNotEmpty()) {
                        val open = list.id in showDone
                        item(key = "done-${list.id}") {
                            Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).clickable(role = Role.Button) {
                                showDone = if (open) showDone - list.id else showDone + list.id
                            }.semantics { stateDescription = if (open) "Expanded" else "Collapsed" }.padding(horizontal = 8.dp),
                                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                LagomIcon(if (open) R.drawable.ic_chevron else R.drawable.ic_chevron_right, size = 16.dp)
                                Text("Completed (${done.size})", color = MaterialTheme.colorScheme.onSurfaceVariant)
                            }
                        }
                        if (open) items(done, key = { it.id }) { DeletableTodoRow(planner, it, words) }
                    }
                }
            }
        }
    }
    adding?.let { list -> NewTaskDialog(planner, list, null) { adding = null } }
    if (newList) NewListDialog(planner) { newList = false }
}

/** A swipe to the start deletes, as iOS's destructive swipe; TalkBack gets the same as an action. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable private fun DeletableTodoRow(planner: PlannerModel, todo: Todo, words: DateWords) {
    val state = rememberSwipeToDismissBoxState()
    // The settled value changes only after release, so a drag that turns back never deletes.
    LaunchedEffect(state.settledValue) {
        if (state.settledValue != SwipeToDismissBoxValue.EndToStart) return@LaunchedEffect
        // In the model's scope, so leaving the page never cuts a delete short. A deleted row leaves with the reload.
        planner.perform { planner.delete(todo) }.join()
        state.reset()
    }
    SwipeToDismissBox(state, enableDismissFromStartToEnd = false, backgroundContent = {
        Row(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.error).padding(horizontal = 20.dp),
            horizontalArrangement = Arrangement.End, verticalAlignment = Alignment.CenterVertically) {
            Text("Delete", color = MaterialTheme.colorScheme.onError)
        }
    }) {
        Surface {
            TodoRow(todo, words, Modifier.semantics {
                customActions = listOf(CustomAccessibilityAction("Delete") { planner.perform { planner.delete(todo) }; true })
            }) { planner.perform { planner.toggle(todo) } }
        }
    }
}

@Composable private fun NewListDialog(planner: PlannerModel, close: () -> Unit) {
    var name by rememberSaveable { mutableStateOf("") }
    AlertDialog(onDismissRequest = close, title = { Text("New list") }, text = {
        OutlinedTextField(name, { name = it }, label = { Text("Name") }, singleLine = true, modifier = Modifier.fillMaxWidth())
    }, confirmButton = { TextButton(onClick = { close(); planner.perform { planner.addList(name) } }) { Text("Add") } },
        dismissButton = { TextButton(onClick = close) { Text("Cancel") } })
}

/** iOS `NewTaskView`: a title, the list when there are several, and an optional due date. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable private fun NewTaskDialog(planner: PlannerModel, startList: Long?, startDue: LocalDate?, close: () -> Unit) {
    var title by rememberSaveable { mutableStateOf("") }
    var list by rememberSaveable { mutableStateOf(startList) }
    // As an epoch day, which saved state can hold.
    var dueDay by rememberSaveable { mutableStateOf(startDue?.toEpochDay()) }
    val due = dueDay?.let(LocalDate::ofEpochDay)
    var saving by remember { mutableStateOf(false) }
    var picking by remember { mutableStateOf(false) }
    var choosing by remember { mutableStateOf(false) }
    val focus = remember { FocusRequester() }
    val canSave = !saving && list != null && title.isNotBlank()
    fun save() {
        val target = list ?: return
        if (!canSave) return
        saving = true
        planner.perform { try { if (planner.add(title, target, due)) close() } finally { saving = false } }
    }
    AlertDialog(onDismissRequest = { if (!saving) close() }, title = { Text("New task") }, text = {
        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlinedTextField(title, { title = it }, label = { Text("Task") }, singleLine = true,
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done), keyboardActions = KeyboardActions(onDone = { save() }),
                modifier = Modifier.fillMaxWidth().focusRequester(focus))
            if (planner.lists.size > 1) Row(verticalAlignment = Alignment.CenterVertically) {
                Text("List", Modifier.weight(1f))
                Box {
                    TextButton(onClick = { choosing = true }, Modifier.heightIn(min = 48.dp)) {
                        Text(planner.lists.firstOrNull { it.id == list }?.name.orEmpty())
                    }
                    DropdownMenu(choosing, { choosing = false }) {
                        planner.lists.forEach { option ->
                            DropdownMenuItem(text = { Text(option.name) }, onClick = { choosing = false; list = option.id })
                        }
                    }
                }
            }
            Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).toggleable(due != null, role = Role.Switch) { dueDay = if (it) (due ?: LocalDate.now()).toEpochDay() else null },
                verticalAlignment = Alignment.CenterVertically) {
                Text("Due date", Modifier.weight(1f))
                Switch(due != null, onCheckedChange = null)
            }
            due?.let { date ->
                TextButton(onClick = { picking = true }, Modifier.heightIn(min = 48.dp).semantics { contentDescription = "Due, ${localDate(date, "EEEEdMMMMyyyy")}" }) {
                    Text(localDate(date, "EEEEdMMMMyyyy"))
                }
            }
        }
    }, confirmButton = { TextButton(enabled = canSave, onClick = ::save) { Text("Add task") } },
        dismissButton = { TextButton(enabled = !saving, onClick = close) { Text("Cancel") } })
    LaunchedEffect(Unit) { focus.requestFocus() }
    if (picking) {
        // The picker counts in UTC midnights.
        val state = rememberDatePickerState(initialSelectedDateMillis = (due ?: LocalDate.now()).atStartOfDay(ZoneOffset.UTC).toInstant().toEpochMilli())
        DatePickerDialog(onDismissRequest = { picking = false }, confirmButton = {
            TextButton(onClick = {
                state.selectedDateMillis?.let { dueDay = Instant.ofEpochMilli(it).atZone(ZoneOffset.UTC).toLocalDate().toEpochDay() }
                picking = false
            }) { Text("OK") }
        }, dismissButton = { TextButton(onClick = { picking = false }) { Text("Cancel") } }) { DatePicker(state) }
    }
}

/** Cached events by day under the month title, as iOS's phone Calendar. A tablet gets iPad's week grid instead. */
@Composable internal fun CalendarScreen(planner: PlannerModel, grid: Boolean = false) {
    if (grid) return WeekGrid(planner)
    val now = nowSeconds()
    val today = LocalDate.now()
    val words = DateWords()
    Column(Modifier.fillMaxSize()) {
        TopBar(Modifier.statusBarsPadding().semantics(mergeDescendants = true) { heading() }) {
            Text(localDate(today, "MMMM"), style = MaterialTheme.typography.titleLarge)
            Text(localDate(today, "yyyy"), Modifier.padding(start = 2.dp), style = MaterialTheme.typography.titleLarge.copy(fontWeight = FontWeight.Normal),
                color = lagom.ink2)
        }
        Refreshable(planner, { planner.reloadAgenda() }, Modifier.weight(1f)) {
            LazyColumn(Modifier.fillMaxSize().padding(horizontal = 16.dp)) {
                if (planner.agenda.isEmpty()) item(key = "empty") {
                    if (planner.calendarConnected) EmptyState(R.drawable.ic_calendar, "Nothing scheduled", Modifier.fillParentMaxSize(), "No events in the next two weeks.")
                    else EmptyState(R.drawable.ic_calendar, "Calendar needs a Google account", Modifier.fillParentMaxSize(), CalendarNeedsGoogle)
                }
                // Core lists events by day, so grouping keeps the days in order.
                planner.agenda.groupBy { it.day }.forEach { (day, events) ->
                    item(key = "day-$day") { DayHeader(day, words) }
                    items(events) { EventRow(planner, it, now) }
                }
            }
        }
    }
}

/** "Mon 9" as the desktop week's column heads, today's number in a filled accent circle. */
@Composable private fun DayHeader(value: String, words: DateWords) {
    val day = words.day(value) ?: return
    val today = day == LocalDate.now()
    val colors = MaterialTheme.colorScheme
    Row(Modifier.padding(top = 14.dp, bottom = 2.dp).clearAndSetSemantics { contentDescription = words.dayTitle(value); heading() },
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(localDate(day, "EEE"), color = colors.onSurfaceVariant)
        Box(Modifier.sizeIn(minWidth = 38.dp, minHeight = 38.dp).background(if (today) colors.primary else Color.Transparent, CircleShape),
            contentAlignment = Alignment.Center) {
            Text(day.dayOfMonth.toString(), style = MaterialTheme.typography.titleMedium, color = if (today) colors.onPrimary else colors.onSurface)
        }
    }
}

private const val CalendarNeedsGoogle =
    "lagom shows your Google Calendar here. Google Calendar sync is coming to Android; mail accounts with a password have no calendar."

private val HourHeight = 60.dp
private val Gutter = 48.dp

/** iPad's week, desktop `week::week_view`: day columns on an hour grid, all-day events above, the now line across. */
@Composable private fun WeekGrid(planner: PlannerModel) {
    val days = (0L until 7L).map { planner.weekStart.plusDays(it) }
    val byDay = planner.week.groupBy { it.day }
    var shown by remember { mutableStateOf<AgendaItem?>(null) }
    Column(Modifier.fillMaxSize().padding(horizontal = 24.dp)) {
        WeekHeader(planner)
        if (!planner.calendarConnected && planner.week.isEmpty()) {
            EmptyState(R.drawable.ic_calendar, "Calendar needs a Google account", Modifier.fillMaxSize(), CalendarNeedsGoogle)
        } else {
            Row(Modifier.padding(top = 8.dp)) {
                Spacer(Modifier.width(Gutter))
                days.forEach { WeekDayHead(it, Modifier.weight(1f)) }
            }
            val allDay = days.map { day -> byDay[day.toString()].orEmpty().filter { it.allDay } }
            // Many all-day events scroll in a band, as desktop's strip, so the hours stay visible.
            if (allDay.any { it.isNotEmpty() }) Row(Modifier.padding(top = 8.dp).heightIn(max = 110.dp).verticalScroll(rememberScrollState())) {
                Text("All day", Modifier.width(Gutter), color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.labelSmall)
                allDay.forEach { events ->
                    Column(Modifier.weight(1f).padding(horizontal = 2.dp), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                        events.forEach { event -> WeekEvent(event, compact = true) { shown = event } }
                    }
                }
            }
            Refreshable(planner, { planner.reloadAgenda() }, Modifier.weight(1f).padding(top = 8.dp)) {
                HourGrid(days, byDay) { shown = it }
            }
        }
    }
    shown?.let { EventDetails(planner, it) { shown = null } }
}

/** The month of the week's Thursday, as ISO weeks count, and the week buttons. */
@Composable private fun WeekHeader(planner: PlannerModel) {
    val thursday = planner.weekStart.plusDays(3)
    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Row(Modifier.semantics(mergeDescendants = true) { heading() }, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(localDate(thursday, "MMMM"), style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.Bold)
            Text(localDate(thursday, "yyyy"), style = MaterialTheme.typography.titleLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Spacer(Modifier.weight(1f))
        LagomIconButton("Previous week", R.drawable.ic_chevron_left) { planner.perform { planner.showWeek(-1) } }
        HouseButton("Today") { planner.perform { planner.showWeek(0) } }
        LagomIconButton("Next week", R.drawable.ic_chevron_right) { planner.perform { planner.showWeek(1) } }
    }
}

/** "Mon 9", weekends quieter, today in the accent circle. */
@Composable private fun WeekDayHead(day: LocalDate, modifier: Modifier) {
    val colors = MaterialTheme.colorScheme
    val today = day == LocalDate.now()
    val weekend = day.dayOfWeek.value >= 6
    Row(modifier.clearAndSetSemantics { contentDescription = localDate(day, "EEEEdMMMM"); heading() },
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        Text(localDate(day, "EEE"), color = colors.onSurfaceVariant)
        Box(Modifier.sizeIn(minWidth = 32.dp, minHeight = 32.dp).background(if (today) colors.primary else Color.Transparent, CircleShape),
            contentAlignment = Alignment.Center) {
            Text(day.dayOfMonth.toString(), style = MaterialTheme.typography.titleMedium,
                color = if (today) colors.onPrimary else if (weekend) colors.onSurfaceVariant else colors.onSurface)
        }
    }
}

/** The hours with each day's timed events; opens at the week's first event, or 08:00 on an empty week. */
@Composable private fun HourGrid(days: List<LocalDate>, byDay: Map<String, List<AgendaItem>>, onShow: (AgendaItem) -> Unit) {
    val blocks = days.map { weekBlocks(byDay[it.toString()].orEmpty(), it) }
    val first = blocks.flatten().minOfOrNull { it.start }?.let { maxOf(0, (it / 60).toInt() - 1) } ?: 8
    val scroll = rememberScrollState()
    val hour = with(LocalDensity.current) { HourHeight.roundToPx() }
    LaunchedEffect(days.first(), first) { scroll.scrollTo(first * hour) }
    val colors = MaterialTheme.colorScheme
    Box(Modifier.fillMaxSize().verticalScroll(scroll).padding(top = 12.dp)) {
        Row {
            Column(Modifier.width(Gutter).clearAndSetSemantics {}) {
                (0 until 24).forEach { h ->
                    Row(Modifier.height(HourHeight).fillMaxWidth().offset(y = (-8).dp).padding(end = 8.dp), horizontalArrangement = Arrangement.End) {
                        Text(h.toString().padStart(2, '0'), color = colors.onSurface, style = MaterialTheme.typography.bodyMedium)
                        Text("00", color = colors.onSurfaceVariant, style = MaterialTheme.typography.labelSmall)
                    }
                }
            }
            days.forEachIndexed { i, day -> DayColumn(day, byDay[day.toString()].orEmpty(), blocks[i], Modifier.weight(1f), onShow) }
        }
        NowLine24(days, Modifier.matchParentSize())
    }
}

@Composable private fun DayColumn(day: LocalDate, events: List<AgendaItem>, blocks: List<WeekBlock>, modifier: Modifier, onShow: (AgendaItem) -> Unit) {
    val colors = MaterialTheme.colorScheme
    val fill = when {
        day == LocalDate.now() -> colors.primary.copy(alpha = 0.06f)
        day.dayOfWeek.value >= 6 -> colors.surfaceVariant
        else -> colors.background
    }
    BoxWithConstraints(modifier.height(HourHeight * 24).background(fill)) {
        (0 until 24).forEach { h -> Box(Modifier.offset(y = HourHeight * h).fillMaxWidth().height(1.dp).background(colors.outlineVariant)) }
        Box(Modifier.fillMaxHeight().width(1.dp).background(colors.outlineVariant))
        blocks.forEach { block ->
            val width = (maxWidth - 3.dp) / block.lanes
            val minutes = (block.end - block.start).toFloat()
            WeekEvent(events[block.index], compact = minutes < 45, Modifier
                .offset(x = 3.dp + width * block.lane, y = HourHeight * (block.start / 60).toFloat() + 1.dp)
                .size(width - 2.dp, HourHeight * (minutes / 60) - 2.dp)) { onShow(events[block.index]) }
        }
    }
}

/** An event on the grid, desktop `event_block`: a tint and bar, the title, then the time. A tap shows its details. */
@Composable private fun WeekEvent(event: AgendaItem, compact: Boolean, modifier: Modifier = Modifier, onClick: () -> Unit) {
    val colors = MaterialTheme.colorScheme
    val tint = colors.primary
    val title = event.title.ifEmpty { "(No title)" }
    val start = clock(event.startUnix)
    val end = clock(maxOf(event.startUnix, event.endUnix))
    Box(modifier.fillMaxWidth().clip(RoundedCornerShape(4.dp)).background(tint.copy(alpha = 0.14f))
        .drawBehind { drawRect(tint, size = Size(2.dp.toPx(), size.height)) }
        .clickable(onClickLabel = "Show details", onClick = onClick)
        .semantics {
            contentDescription = listOfNotNull(if (event.allDay) "All day" else null, title, event.location?.takeIf { it.isNotEmpty() }).joinToString(", ")
            if (!event.allDay) stateDescription = "$start – $end"
        }
        .padding(start = 7.dp, end = 5.dp, top = 2.dp, bottom = 2.dp)) {
        Column(Modifier.clearAndSetSemantics {}) {
            Text(if (compact && !event.allDay) "$title · $start" else title, style = MaterialTheme.typography.labelSmall, fontWeight = FontWeight.Medium,
                maxLines = if (compact) 1 else 2, overflow = TextOverflow.Ellipsis)
            if (!compact && !event.allDay) Text(start, color = colors.onSurfaceVariant, style = MaterialTheme.typography.labelSmall.copy(fontFeatureSettings = "tnum"), maxLines = 1)
        }
    }
}

/** iPad's event popover: title, when, where, the calendar and a meeting link. */
@Composable private fun EventDetails(planner: PlannerModel, event: AgendaItem, close: () -> Unit) {
    val context = LocalContext.current
    val colors = MaterialTheme.colorScheme
    val day = Instant.ofEpochSecond(event.startUnix).atZone(ZoneId.systemDefault()).toLocalDate()
    val start = clock(event.startUnix)
    val end = clock(maxOf(event.startUnix, event.endUnix))
    AlertDialog(onDismissRequest = close, title = { Text(event.title.ifEmpty { "(No title)" }) }, text = {
        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(if (event.allDay) "All day" else "${localDate(day, "EEEdMMMM")}, $start – $end", color = colors.onSurfaceVariant,
                style = MaterialTheme.typography.bodySmall)
            event.location?.takeIf { it.isNotEmpty() }?.let { Text(it) }
            Text(event.calendar, color = colors.onSurfaceVariant, style = MaterialTheme.typography.bodySmall)
        }
    }, confirmButton = {
        event.webLink()?.let { link -> TextButton(onClick = { openLink(context, planner, link) }, Modifier.heightIn(min = 48.dp)) { Text("Join meeting") } }
    }, dismissButton = { TextButton(onClick = close, Modifier.heightIn(min = 48.dp)) { Text("Close") } })
}

/** The now line across the week, desktop `current-time`: the time in the gutter, a dot on today's column. */
@Composable private fun NowLine24(days: List<LocalDate>, modifier: Modifier) {
    val now = nowSeconds()
    val time = Instant.ofEpochSecond(now).atZone(ZoneId.systemDefault())
    val today = days.indexOf(time.toLocalDate())
    if (today < 0) return
    val gold = lagom.later
    val label = clock(now)
    val y = HourHeight * (time.hour + time.minute / 60f)
    BoxWithConstraints(modifier.clearAndSetSemantics { contentDescription = "Now, $label" }) {
        val column = (maxWidth - Gutter) / days.size
        Box(Modifier.offset(x = Gutter, y = y).width(maxWidth - Gutter).height(1.5.dp).background(gold))
        Box(Modifier.offset(x = Gutter + column * today - 3.5.dp, y = y - 2.75.dp).size(7.dp).background(gold, CircleShape))
        Text(label, Modifier.offset(y = y - 9.dp).width(Gutter - 4.dp).background(gold, RoundedCornerShape(4.dp)),
            color = lagom.accentInk, style = MaterialTheme.typography.labelSmall.copy(fontFeatureSettings = "tnum"), textAlign = TextAlign.Center, maxLines = 1)
    }
}
