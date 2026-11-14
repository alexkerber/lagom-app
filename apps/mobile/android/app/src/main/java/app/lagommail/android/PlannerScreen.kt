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
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.addPathNodes
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.*
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.lagommail.core.*
import kotlinx.coroutines.delay
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneOffset
import java.util.Date

// Material's outlined icons for the page bar; the app has no icon library.
private fun icon(name: String, path: String) = ImageVector.Builder(name, 24.dp, 24.dp, 24f, 24f)
    .addPath(addPathNodes(path), fill = SolidColor(Color.Black)).build()
internal val SectionIcons = mapOf(
    AppSection.TODAY to icon("Today", "M12 5.69l5 4.5V18h-2v-6H9v6H7v-7.81l5-4.5M12 3L2 12h3v8h6v-6h2v6h6v-8h3L12 3z"),
    AppSection.MAIL to icon("Mail", "M22 6c0-1.1-.9-2-2-2H4c-1.1 0-2 .9-2 2v12c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V6zm-2 0l-8 5-8-5h16zm0 12H4V8l8 5 8-5v10z"),
    AppSection.CALENDAR to icon("Calendar", "M20 3h-1V1h-2v2H7V1H5v2H4c-1.1 0-2 .9-2 2v16c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2zm0 18H4V10h16v11zm0-13H4V5h16v3z"),
    AppSection.TODO to icon("Todo", "M19 3H5c-1.11 0-2 .9-2 2v14c0 1.1.89 2 2 2h14c1.11 0 2-.9 2-2V5c0-1.1-.89-2-2-2zm0 16H5V5h14v14zM17.99 9l-1.41-1.42-6.59 6.59-2.58-2.57-1.42 1.41 4 3.99z"),
)

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
    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).padding(start = 16.dp, end = 4.dp, top = 8.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(title, Modifier.semantics { heading() }, style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.SemiBold)
        Spacer(Modifier.weight(1f))
        trailing()
    }
}

/** Schedule, To do, Mail, a task list's name. */
@Composable private fun Heading(title: String, modifier: Modifier = Modifier) =
    Text(title, modifier.padding(bottom = 8.dp).semantics { heading() }, style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.SemiBold)

@Composable private fun Quiet(text: String) =
    Text(text, color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodyMedium)

/** "+ New task", "+ Add task": a quiet line that adds. */
@Composable private fun AddRow(title: String, enabled: Boolean = true, onClick: () -> Unit) {
    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).clickable(enabled = enabled, role = Role.Button, onClick = onClick).padding(horizontal = 8.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        val ink = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = if (enabled) 1f else 0.38f)
        Text("+", Modifier.width(22.dp).clearAndSetSemantics {}, color = ink, textAlign = TextAlign.Center)
        Text(title, color = ink)
    }
}

/** The desktop's empty pages: a quiet title and line in the middle. */
@Composable private fun EmptyState(title: String, detail: String, modifier: Modifier = Modifier) {
    Column(modifier.padding(24.dp), verticalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterVertically),
        horizontalAlignment = Alignment.CenterHorizontally) {
        Text(title, style = MaterialTheme.typography.titleMedium, textAlign = TextAlign.Center)
        Text(detail, color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = TextAlign.Center)
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
@Composable internal fun TodayScreen(planner: PlannerModel, model: MailModel, openMail: (Long?) -> Unit) {
    var newTask by rememberSaveable { mutableStateOf(false) }
    val now = nowSeconds()
    val words = DateWords()
    Column(Modifier.fillMaxSize()) {
        PageTitle("Today") {
            Text(localDate(LocalDate.now(), "EEEEdMMMM"), Modifier.padding(end = 12.dp),
                color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodySmall)
        }
        Refreshable(planner, { planner.reload() }, Modifier.weight(1f)) {
            Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(32.dp)) {
                Column {
                    Heading("Schedule")
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
                    Heading("To do")
                    val tasks = planner.today?.tasks.orEmpty()
                    if (tasks.isEmpty()) Quiet("Nothing due today.")
                    tasks.forEach { todo -> TodoRow(todo, words) { planner.perform { planner.toggle(todo) } } }
                    AddRow("New task", enabled = planner.lists.isNotEmpty()) { newTask = true }
                }
                Column {
                    Heading("Mail")
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
        if (planner.unreadCount > 0) TextButton(onClick = { openMail(null) }, Modifier.padding(horizontal = 4.dp).heightIn(min = 48.dp)) {
            Text("${planner.unreadCount} unread in Inbox  ›", Modifier.semantics { contentDescription = "${planner.unreadCount} unread in Inbox" }, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
    if (newTask) NewTaskDialog(planner, planner.defaultList, LocalDate.now()) { newTask = false }
}

/** The current time in gold across the schedule. */
@Composable private fun NowLine(now: Long) {
    val time = clock(now)
    val gold = MaterialTheme.colorScheme.secondary
    Row(Modifier.fillMaxWidth().heightIn(min = 28.dp).clearAndSetSemantics { contentDescription = "Now, $time" },
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(time, Modifier.widthIn(min = 52.dp), color = gold, style = MaterialTheme.typography.labelSmall)
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
            color = colors.onSurfaceVariant, style = MaterialTheme.typography.labelSmall)
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
                if (todo.done) Icon(CheckIcon, contentDescription = null, Modifier.size(16.dp), tint = colors.onSurfaceVariant)
            }
        }
        Text(todo.title, Modifier.weight(1f), color = if (todo.done) colors.onSurfaceVariant else colors.onSurface,
            textDecoration = if (todo.done) TextDecoration.LineThrough else null)
        if (todo.fromMail) Icon(SectionIcons.getValue(AppSection.MAIL), contentDescription = "From mail", Modifier.size(16.dp), tint = colors.onSurfaceVariant)
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
                TextButton(onClick = { menu = true }, Modifier.sizeIn(minWidth = 48.dp, minHeight = 48.dp).semantics { contentDescription = "Add" }) {
                    Text("+", style = MaterialTheme.typography.titleLarge)
                }
                DropdownMenu(menu, { menu = false }) {
                    DropdownMenuItem(text = { Text("New task") }, enabled = planner.lists.isNotEmpty(), onClick = { menu = false; adding = planner.defaultList })
                    DropdownMenuItem(text = { Text("New list") }, onClick = { menu = false; newList = true })
                }
            }
        }
        Refreshable(planner, { planner.reloadTodos() }, Modifier.weight(1f)) {
            LazyColumn(Modifier.fillMaxSize().padding(horizontal = 16.dp)) {
                if (planner.lists.isEmpty()) item(key = "empty") { EmptyState("No lists", "Add a list to start.", Modifier.fillParentMaxSize()) }
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
                                Text(if (open) "⌄" else "›", Modifier.clearAndSetSemantics {}, color = MaterialTheme.colorScheme.onSurfaceVariant)
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

/** Cached events by day under the month title, as iOS's phone Calendar. */
@Composable internal fun CalendarScreen(planner: PlannerModel) {
    val now = nowSeconds()
    val today = LocalDate.now()
    val words = DateWords()
    Column(Modifier.fillMaxSize()) {
        Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).padding(start = 16.dp, top = 8.dp).semantics(mergeDescendants = true) { heading() },
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(localDate(today, "MMMM"), style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.Bold)
            Text(localDate(today, "yyyy"), style = MaterialTheme.typography.titleLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Refreshable(planner, { planner.reloadAgenda() }, Modifier.weight(1f)) {
            LazyColumn(Modifier.fillMaxSize().padding(horizontal = 16.dp)) {
                if (planner.agenda.isEmpty()) item(key = "empty") {
                    if (planner.calendarConnected) EmptyState("Nothing scheduled", "No events in the next two weeks.", Modifier.fillParentMaxSize())
                    else EmptyState("Calendar needs a Google account",
                        "lagom shows your Google Calendar here. Google Calendar sync is coming to Android; mail accounts with a password have no calendar.",
                        Modifier.fillParentMaxSize())
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
