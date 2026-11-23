package app.lagommail.android

import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.Uri
import androidx.activity.compose.BackHandler
import androidx.annotation.DrawableRes
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.EaseOut
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.ui.layout.onSizeChanged
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
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.withLink
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.Dp
import java.time.LocalDateTime
import java.time.LocalTime
import androidx.compose.ui.text.input.KeyboardType
import app.lagommail.core.*
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
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

/** Schedule times: small, with tabular digits. */
@Composable private fun timeStyle() = MaterialTheme.typography.labelSmall.copy(fontFeatureSettings = "tnum")

/** The schedule's time column, as wide as its widest label at this font scale: every hour in this
 *  locale's clock ("11:00 AM" or "23:00"). So every bar lines up. */
@Composable private fun timeColumn(): Dp {
    val context = LocalContext.current
    val density = LocalDensity.current
    val style = timeStyle()
    val measurer = rememberTextMeasurer()
    // The clock setting can change while Today stays composed; the rows read it at every recomposition.
    val twentyFour = android.text.format.DateFormat.is24HourFormat(context)
    return remember(context, density, style, twentyFour) {
        val format = android.text.format.DateFormat.getTimeFormat(context)
        // A fixed date: on a daylight-saving day one hour is missing or repeated.
        val hour = java.util.Calendar.getInstance().apply { set(2026, 0, 1) }
        val labels = (0..23).map { hour.set(java.util.Calendar.HOUR_OF_DAY, it); hour.set(java.util.Calendar.MINUTE, 0); format.format(hour.time) }
        with(density) { labels.maxOf { measurer.measure(it, style, maxLines = 1).size.width }.toDp() }
    }
}

/** A page title as on desktop and iOS: semibold, actions on the right. */
@Composable private fun PageTitle(title: String, trailing: @Composable RowScope.() -> Unit = {}) {
    TopBar(Modifier.statusBarsPadding()) {
        BarTitle(title, Modifier.weight(1f))
        trailing()
    }
}

/** The task circles' column: a list's dot, the add and Completed glyphs sit in it, so every line's words start
 *  where the task titles do. */
@Composable private fun Lead(content: @Composable () -> Unit) = Box(Modifier.width(48.dp), contentAlignment = Alignment.Center) { content() }

/** A task list's heading, "● Work  4": its dot over the task circles, its name over their titles and the tasks still to do. */
@Composable private fun ListHeading(title: String, dot: Color, open: Int, modifier: Modifier = Modifier) {
    Row(modifier.heightIn(min = 40.dp).semantics(mergeDescendants = true) { heading() }, verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        Lead { Box(Modifier.size(8.dp).background(dot, CircleShape)) }
        // The count keeps its room when a long name ellipsizes.
        Text(title, Modifier.weight(1f, fill = false), color = lagom.ink, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.titleMedium.copy(fontFamily = DisplayFace, fontWeight = FontWeight.Bold))
        if (open > 0) Text("$open", Modifier.semantics { contentDescription = "$open open" }, color = lagom.ink2,
            fontFamily = LabelFace, fontWeight = FontWeight.Medium, style = MaterialTheme.typography.labelSmall)
    }
}

/** Today's display sentence, also To-do's "A clear day.". */
@Composable private fun DisplaySentence(text: AnnotatedString, modifier: Modifier = Modifier, color: Color = MaterialTheme.colorScheme.onSurfaceVariant,
    textAlign: TextAlign? = null) {
    val wide = LocalConfiguration.current.screenWidthDp >= 600
    Text(text, modifier.semantics { heading() }, color = color, fontFamily = DisplayFace, textAlign = textAlign,
        fontWeight = FontWeight.Bold, fontSize = if (wide) 48.sp else 34.sp, letterSpacing = DisplayTracking.em, lineHeight = if (wide) 50.sp else 36.sp)
}

/** Small Geist Mono capitals, the site's labels: Today's date and section titles. */
@Composable internal fun MonoLabel(text: String, modifier: Modifier = Modifier) =
    Text(text.uppercase(), modifier.semantics { heading(); contentDescription = text }, color = MaterialTheme.colorScheme.onSurfaceVariant,
        fontFamily = LabelFace, fontWeight = FontWeight.Medium, fontSize = 13.sp)

/** The desktop's Today sentence in two tones (Alex 2026-11-17): the counts are ink and open their page. */
@Composable private fun TodaySentence(planner: PlannerModel, open: (AppSection) -> Unit, openMail: (Long?) -> Unit) {
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
        DisplaySentence(text)
    }
}

@Composable private fun Quiet(text: String) =
    Text(text, color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodyMedium)

/** "+ New task", "+ New list": a quiet line that adds, its plus over the task circles. */
@Composable private fun AddRow(title: String, enabled: Boolean = true, onClick: () -> Unit) {
    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).clickable(enabled = enabled, role = Role.Button, onClick = onClick),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        val ink = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = if (enabled) 1f else 0.38f)
        Lead { LagomIcon(R.drawable.ic_plus, tint = ink) }
        Text(title, color = ink)
    }
}

/** How long a tick shows before its task moves to Completed, so the tap is seen. */
private const val TICK_MILLIS = 280L


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
                    val access = planner.calendarAccess.firstOrNull()
                    if (events.isEmpty()) Quiet(when {
                        planner.calendarConnected -> "Nothing scheduled today."
                        access != null -> "Google Calendar is not connected."
                        planner.calendarGranted -> "Your calendar shows here after the next sync."
                        else -> "Add a Google account to see your calendar here."
                    })
                    // The now line sits before the first timed event still to start, as on desktop.
                    val nowIndex = events.indexOfFirst { !it.allDay && it.startUnix > now }.let { if (it < 0) events.size else it }
                    val column = timeColumn()
                    events.forEachIndexed { index, event ->
                        if (index == nowIndex) NowLine(now, column)
                        EventRow(planner, event, now, column)
                    }
                    if (events.isNotEmpty() && nowIndex == events.size) NowLine(now, column)
                    // Also beside another account's calendar: each Google account allows its own.
                    CalendarAccessButtons(planner, model, planner.calendarAccess, Modifier.padding(top = 8.dp))
                }
                Column {
                    MonoLabel("To-do", Modifier.padding(bottom = 8.dp))
                    val tasks = planner.today?.tasks.orEmpty()
                    if (tasks.isEmpty()) Quiet("Nothing due today.")
                    tasks.forEach { todo -> TodoRow(todo, words) { planner.perform { planner.toggle(todo, TICK_MILLIS) } } }
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
@Composable private fun NowLine(now: Long, column: Dp) {
    val time = clock(now)
    val gold = lagom.later
    Row(Modifier.fillMaxWidth().heightIn(min = 28.dp).clearAndSetSemantics { contentDescription = "Now, $time" },
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(time, Modifier.width(column), color = gold, style = timeStyle(), maxLines = 1)
        Box(Modifier.weight(1f).height(2.dp).background(gold, RoundedCornerShape(1.dp)))
    }
}

/** One schedule line, desktop `agenda_view`: time, a bar, the title, and a meeting link. */
@Composable private fun EventRow(planner: PlannerModel, event: AgendaItem, now: Long, column: Dp) {
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
        // All day is the sun and a dot, a timed event its time and a bar, as on Mac and iOS.
        if (event.allDay) {
            Box(Modifier.width(column), contentAlignment = Alignment.CenterEnd) { LagomIcon(R.drawable.ic_sun, size = 16.dp) }
            Box(Modifier.size(5.dp).background(colors.primary, CircleShape))
        } else {
            Text(start, Modifier.width(column).clearAndSetSemantics {}, color = colors.onSurfaceVariant, style = timeStyle(), maxLines = 1)
            Box(Modifier.width(2.dp).height(28.dp).background(colors.primary, RoundedCornerShape(1.dp)))
        }
        Column(Modifier.weight(1f).clearAndSetSemantics {}) {
            Text(title, color = if (past) colors.onSurfaceVariant else colors.onSurface, maxLines = 1, overflow = TextOverflow.Ellipsis)
            event.location?.takeIf { it.isNotEmpty() }?.let {
                Text(it, color = colors.onSurfaceVariant, style = MaterialTheme.typography.bodySmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
        event.webLink()?.let { link ->
            LagomIconButton("Join meeting", R.drawable.ic_video) { openLink(context, planner, link) }
        }
    }
}

/** A task, desktop `task_row`: a completion circle, its title, a mail mark, and the due chip. A tap ticks the circle at
 *  once; [toggle] saves and reloads a moment later (TICK_MILLIS), so the tick is seen before the row moves. */
@Composable private fun TodoRow(todo: Todo, words: DateWords, modifier: Modifier = Modifier, toggle: () -> Job) {
    val colors = MaterialTheme.colorScheme
    val haptics = LocalHapticFeedback.current
    val scope = rememberCoroutineScope()
    var ticked by remember(todo.id) { mutableStateOf<Boolean?>(null) }
    val done = ticked ?: todo.done
    fun tap() {
        if (ticked != null) return // one change at a time
        ticked = !todo.done
        if (!todo.done) haptics.performHapticFeedback(HapticFeedbackType.ToggleOn)
        // The model's job outlives the row; the reload clears the tick, or brings the circle back if core refused.
        val job = toggle()
        scope.launch { job.join(); ticked = null }
    }
    Row(modifier.fillMaxWidth().heightIn(min = 48.dp).padding(end = 8.dp), verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        // A checkbox named after its task; iOS's words are the action TalkBack offers.
        Box(Modifier.size(48.dp).toggleable(done, role = Role.Checkbox) { tap() }.semantics {
            contentDescription = todo.title
            onClick(if (done) "Mark as not done" else "Mark as done") { tap(); true }
        }, contentAlignment = Alignment.Center) { CheckCircle(done) }
        Text(todo.title, Modifier.weight(1f), color = if (done) colors.onSurfaceVariant else colors.onSurface,
            textDecoration = if (done) TextDecoration.LineThrough else null)
        if (todo.fromMail) LagomIcon(R.drawable.ic_linked_mail, size = 16.dp, label = "From mail")
        val due = if (done) null else todo.due?.let(words::due)
        if (due != null) DueChip(due.first, due.second)
    }
}

/** The completion circle: a hairline ring that fills with Done's pine green and a tick. The fill springs in and fades;
 *  with animations off it is there at once. */
@Composable private fun CheckCircle(done: Boolean) {
    val c = lagom
    val scale by animateFloatAsState(if (done) 1f else 0.5f, landing(), label = "tick scale")
    val shown by animateFloatAsState(if (done) 1f else 0f, Fade, label = "tick fade")
    Box(Modifier.size(21.dp), contentAlignment = Alignment.Center) {
        Box(Modifier.matchParentSize().graphicsLayer { alpha = 1f - shown }.border(1.dp, c.ink2, CircleShape))
        Box(Modifier.matchParentSize().graphicsLayer { alpha = shown; scaleX = scale; scaleY = scale }.background(c.done, CircleShape),
            contentAlignment = Alignment.Center) { LagomIcon(R.drawable.ic_done, size = 15.dp, tint = c.accentInk) }
    }
}

/** A due date, desktop's chip: quiet, or red once overdue. */
@Composable private fun DueChip(label: String, overdue: Boolean = false) {
    val colors = MaterialTheme.colorScheme
    Text(label, Modifier.background(if (overdue) colors.error.copy(alpha = 0.12f) else colors.surfaceVariant, RoundedCornerShape(6.dp))
        .padding(horizontal = 7.dp, vertical = 3.dp).semantics { contentDescription = if (overdue) "Overdue, due $label" else "Due $label" },
        color = if (overdue) colors.error else colors.onSurfaceVariant, style = MaterialTheme.typography.labelSmall)
}

/** The inline field's target when it adds a list rather than a task. */
private const val NEW_LIST = Long.MIN_VALUE

/** Local task lists, as iOS `TodoView` and the desktop's Todo page. Adding happens where the eye is (Alex 2026-11-20,
 *  "plus uppe i hörnet … old school"): "New task" ends each list and turns into a field in place, with due chips above
 *  the keyboard. Enter adds and keeps the field open for the next task; Close or Back keeps what was typed. */
@Composable internal fun TodoScreen(planner: PlannerModel, model: MailModel) {
    // The list the field is open in, NEW_LIST, or null.
    var adding by rememberSaveable { mutableStateOf<Long?>(null) }
    var title by rememberSaveable { mutableStateOf("") }
    // As an epoch day, which saved state can hold.
    var dueDay by rememberSaveable { mutableStateOf<Long?>(null) }
    var picking by remember { mutableStateOf(false) }
    var showDone by remember { mutableStateOf<Set<Long>>(emptySet()) }
    val words = DateWords()
    val due = dueDay?.let(LocalDate::ofEpochDay)
    /** A task core refused comes back into its field, unless that field already holds the next one or its list is gone;
     *  the alert names it either way. */
    fun add(text: String, target: Long, day: Long?) = planner.perform {
        if (!planner.add(text, target, day?.let(LocalDate::ofEpochDay)) && title.isEmpty() && (adding == null || adding == target)
            && planner.lists.any { it.id == target }) { adding = target; title = text; dueDay = day }
    }
    /** Saves what the field holds and closes it. */
    fun commit() {
        val text = title.trim()
        val target = adding
        if (text.isNotEmpty() && target != null) { if (target == NEW_LIST) planner.perform { planner.addList(text) } else add(text, target, dueDay) }
        title = ""; dueDay = null; adding = null
    }
    fun open(target: Long) { commit(); adding = target }
    /** Enter: the task is added and the field stays for the next; an empty Enter, or a list name, closes it. */
    fun submit() {
        val target = adding ?: return
        if (title.isBlank() || target == NEW_LIST) { commit(); return }
        val text = title.trim()
        title = ""
        add(text, target, dueDay)
    }
    BackHandler(enabled = adding != null) { commit() }
    Column(Modifier.fillMaxSize().imePadding()) {
        PageTitle(AppSection.TODO.title)
        // Calendar granted but Google Tasks declined or taken back: as the Mac's To-do, ask here.
        CalendarAccessButtons(planner, model, planner.tasksAccess, Modifier.padding(start = 16.dp, end = 16.dp, bottom = 8.dp), tasks = true)
        Refreshable(planner, { planner.reloadTodos() }, Modifier.weight(1f)) {
            // The circles' column lines up under the page title; the due chips end on its 16 dp margin.
            LazyColumn(Modifier.fillMaxSize().padding(start = 4.dp, end = 8.dp)) {
                if (planner.lists.isEmpty() && adding != NEW_LIST) item(key = "empty") {
                    EmptyState(R.drawable.ic_todo, "No lists yet", Modifier.fillParentMaxSize(), "A list holds what there is to do.") {
                        HouseButton("New list", R.drawable.ic_plus) { open(NEW_LIST) }
                    }
                }
                // The lists and their New task lines stay below, so adding is still one tap away.
                else if (planner.lists.isNotEmpty() && planner.lists.all { planner.openCount(it.id) == 0 }) item(key = "clear") {
                    Column(Modifier.fillMaxWidth().padding(start = 12.dp, end = 12.dp, top = 24.dp, bottom = 24.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        DisplaySentence(AnnotatedString("Nothing to do.\nA clear day."), color = lagom.ink)
                        Quiet("Add a task below when something comes up.")
                    }
                }
                planner.lists.forEachIndexed { index, list ->
                    val all = planner.todos[list.id].orEmpty()
                    val done = all.filter { it.done }
                    // shortcut: the dot follows list order, so it moves when lists are added or removed; store a colour if people ask to choose one.
                    item(key = "list-${list.id}") {
                        ListHeading(list.name, accountAccent(index), all.size - done.size, Modifier.padding(top = if (index == 0) 8.dp else 24.dp))
                    }
                    items(all.filter { !it.done }, key = { it.id }) { SwipeTodoRow(planner, it, words, Modifier.animateItem()) }
                    item(key = "add-${list.id}") {
                        if (adding == list.id) AddField(title, { title = it }, false, due?.let { words.due(it.toString())?.first }, ::submit) {
                            // Only this field's own leaving: moving to another list's field has already saved it.
                            if (!picking && adding == list.id) commit()
                        }
                        else AddRow("New task") { open(list.id) }
                    }
                    if (done.isNotEmpty()) {
                        val expanded = list.id in showDone
                        item(key = "done-${list.id}") {
                            Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).clickable(role = Role.Button) {
                                showDone = if (expanded) showDone - list.id else showDone + list.id
                            }.semantics { stateDescription = if (expanded) "Expanded" else "Collapsed" },
                                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                                Lead { LagomIcon(if (expanded) R.drawable.ic_chevron else R.drawable.ic_chevron_right, size = 16.dp) }
                                Text("Completed (${done.size})", color = MaterialTheme.colorScheme.onSurfaceVariant)
                            }
                        }
                        if (expanded) items(done, key = { it.id }) { SwipeTodoRow(planner, it, words, Modifier.animateItem()) }
                    }
                }
                item(key = "new-list") {
                    Box(Modifier.padding(top = 16.dp)) {
                        if (adding == NEW_LIST) AddField(title, { title = it }, true, null, ::submit) { if (adding == NEW_LIST) commit() }
                        else if (planner.lists.isNotEmpty()) AddRow("New list") { open(NEW_LIST) }
                    }
                }
            }
        }
        if (adding != null && adding != NEW_LIST) DueBar(due, { dueDay = it?.toEpochDay() }, { picking = true }) { commit() }
    }
    if (picking) {
        // The picker counts in UTC midnights.
        val today = LocalDate.now().atStartOfDay(ZoneOffset.UTC).toInstant().toEpochMilli()
        // From today on, as on iPhone.
        val state = rememberDatePickerState(initialSelectedDateMillis = (due ?: LocalDate.now().plusDays(1)).atStartOfDay(ZoneOffset.UTC).toInstant().toEpochMilli(),
            selectableDates = object : SelectableDates { override fun isSelectableDate(utcTimeMillis: Long) = utcTimeMillis >= today })
        DatePickerDialog(onDismissRequest = { picking = false }, confirmButton = {
            TextButton(onClick = {
                state.selectedDateMillis?.let { dueDay = Instant.ofEpochMilli(it).atZone(ZoneOffset.UTC).toLocalDate().toEpochDay() }
                picking = false
            }) { Text("OK") }
        }, dismissButton = { TextButton(onClick = { picking = false }) { Text("Cancel") } }) { DatePicker(state) }
    }
}

/** "New task" (or "New list") as a field in place: an empty circle (or the list dot) where the row will be. */
@Composable private fun AddField(value: String, onValue: (String) -> Unit, list: Boolean, due: String?, submit: () -> Unit, leave: () -> Unit) {
    val c = lagom
    val focus = remember { FocusRequester() }
    var focused by remember { mutableStateOf(false) }
    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).padding(end = 8.dp), verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        Lead {
            if (list) Box(Modifier.size(8.dp).background(c.line, CircleShape))
            else Box(Modifier.size(21.dp).border(1.dp, c.line, CircleShape))
        }
        val style = if (list) MaterialTheme.typography.titleMedium.copy(fontFamily = DisplayFace, fontWeight = FontWeight.Bold, color = c.ink)
            else MaterialTheme.typography.bodyLarge.copy(color = c.ink)
        BasicTextField(value, onValue, Modifier.weight(1f).focusRequester(focus).onFocusChanged {
            // Leaving the field keeps what was typed.
            if (focused && !it.isFocused) leave()
            focused = it.isFocused
        }.semantics { contentDescription = if (list) "New list name" else "New task" }, textStyle = style, singleLine = true,
            cursorBrush = SolidColor(c.accent),
            // A custom Done keeps the keyboard up for the next task.
            keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences, imeAction = ImeAction.Done),
            keyboardActions = KeyboardActions(onDone = { submit() }),
            decorationBox = { inner ->
                Box { if (value.isEmpty()) Text(if (list) "List name" else "New task", style = style.copy(color = c.ink2)); inner() }
            })
        due?.let { DueChip(it) }
    }
    LaunchedEffect(Unit) { focus.requestFocus() }
}

/** Due chips above the keyboard while a task is typed: Today, Tomorrow, or a date. */
@Composable private fun DueBar(due: LocalDate?, choose: (LocalDate?) -> Unit, pick: () -> Unit, close: () -> Unit) {
    val c = lagom
    val today = LocalDate.now()
    val tomorrow = today.plusDays(1)
    val other = due != null && due != today && due != tomorrow
    Row(Modifier.fillMaxWidth().background(c.bg).drawHairline(c.line, top = true).padding(start = 16.dp, end = 4.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        DueOption("Today", due == today) { choose(if (due == today) null else today) }
        DueOption("Tomorrow", due == tomorrow) { choose(if (due == tomorrow) null else tomorrow) }
        DueOption(if (other) DateWords().due(due.toString())?.first ?: "Date" else "Date", other, R.drawable.ic_calendar, pick)
        Spacer(Modifier.weight(1f))
        LagomIconButton("Close", R.drawable.ic_close, onClick = close)
    }
}

@Composable private fun DueOption(text: String, on: Boolean, @DrawableRes icon: Int? = null, onClick: () -> Unit) {
    val c = lagom
    val ink = if (on) c.accentFillInk else c.ink
    Row(Modifier.minimumInteractiveComponentSize().heightIn(min = 32.dp).clip(RoundedCornerShape(Radius.control))
        .background(if (on) c.accentFill else c.subtle).clickable(role = Role.Button, onClick = onClick)
        .semantics { selected = on; contentDescription = "Due $text" }.padding(horizontal = 10.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        icon?.let { LagomIcon(it, size = 16.dp, tint = ink) }
        Text(text, color = ink, style = MaterialTheme.typography.labelLarge, maxLines = 1)
    }
}

/** As Mail: right past 120 dp is Done (Not done in Completed), left past 60 % of the row is Delete; tasks have no
 *  Later. The colour behind shows the move a release makes. TalkBack gets Delete as an action. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable private fun SwipeTodoRow(planner: PlannerModel, todo: Todo, words: DateWords, modifier: Modifier = Modifier) {
    val reach = with(LocalDensity.current) { 120.dp.toPx() }
    // The threshold depends on the direction, which only the state knows.
    val holder = remember { arrayOfNulls<SwipeToDismissBoxState>(1) }
    val width = remember { floatArrayOf(0f) }
    val state = rememberSwipeToDismissBoxState(confirmValueChange = { value ->
        // A quick flick must still cover Mail's distance; the box alone would settle on speed.
        val stage = holder[0]?.let { swipeStage(it.requireOffset(), width[0], reach, true) }
        when (value) {
            SwipeToDismissBoxValue.StartToEnd -> stage == MailAction.Done
            SwipeToDismissBoxValue.EndToStart -> stage == MailAction.Delete
            SwipeToDismissBoxValue.Settled -> true
        }
    }, positionalThreshold = { total ->
        if (holder[0]?.dismissDirection == SwipeToDismissBoxValue.EndToStart) total * 0.6f else reach
    })
    holder[0] = state
    val haptics = LocalHapticFeedback.current
    LaunchedEffect(state.targetValue) {
        if (state.targetValue != SwipeToDismissBoxValue.Settled) haptics.performHapticFeedback(HapticFeedbackType.GestureThresholdActivate)
    }
    // The settled value changes only after release, so a drag that turns back never acts.
    LaunchedEffect(state.settledValue) {
        when (state.settledValue) {
            // In the model's scope, so leaving the page never cuts the change short. The row moves with the reload.
            SwipeToDismissBoxValue.EndToStart -> planner.perform { planner.delete(todo) }.join()
            SwipeToDismissBoxValue.StartToEnd -> planner.perform { planner.toggle(todo) }.join()
            SwipeToDismissBoxValue.Settled -> return@LaunchedEffect
        }
        state.reset()
    }
    SwipeToDismissBox(state, modifier = modifier.onSizeChanged { width[0] = it.width.toFloat() }, backgroundContent = {
        val c = lagom
        val right = state.dismissDirection == SwipeToDismissBoxValue.StartToEnd
        val acting = state.targetValue != SwipeToDismissBoxValue.Settled
        val fill by animateColorAsState(if (!acting) c.subtle else if (right) c.done else c.danger, tween(100, easing = EaseOut), label = "swipe fill")
        val ink = if (acting) c.accentInk else c.ink2
        Row(Modifier.fillMaxSize().background(fill).padding(horizontal = 20.dp).clearAndSetSemantics {},
            horizontalArrangement = if (right) Arrangement.Start else Arrangement.End, verticalAlignment = Alignment.CenterVertically) {
            LagomIcon(if (right) R.drawable.ic_done else R.drawable.ic_trash, tint = ink, size = 20.dp)
            Spacer(Modifier.width(8.dp))
            Text(if (!right) "Delete" else if (todo.done) "Not done" else "Done", color = ink, style = MaterialTheme.typography.labelLarge)
        }
    }) {
        Surface(color = lagom.bg) {
            TodoRow(todo, words, Modifier.semantics {
                customActions = listOf(CustomAccessibilityAction("Delete") { planner.perform { planner.delete(todo) }; true })
            }) { planner.perform { planner.toggle(todo, TICK_MILLIS) } }
        }
    }
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

/** A week strip over one day on phones; tablets keep the full week, as on iOS. */
@Composable internal fun CalendarScreen(planner: PlannerModel, model: MailModel, grid: Boolean = false) {
    WeekGrid(planner, model, oneDay = !grid)
}

/** Desktop's "Show your Google Calendar": Google asks once in the browser, then the redirect's sync fills Calendar, Today
 *  and To-do. One per account that needs it; with more than one each names its address, since Google must be answered
 *  with that account. [tasks] is To-do's grant for Google Tasks; Calendar's asks for both. */
@Composable private fun CalendarAccessButtons(planner: PlannerModel, model: MailModel, accounts: List<MailAccount>, modifier: Modifier = Modifier,
    tasks: Boolean = false) {
    if (accounts.isEmpty()) return
    val context = LocalContext.current
    // A second tap while the browser opens would replace the pending sign-in and fail the first redirect.
    var opening by remember { mutableStateOf(false) }
    LaunchedEffect(opening) { if (opening) { delay(2_000); opening = false } }
    Column(modifier, verticalArrangement = Arrangement.spacedBy(8.dp)) {
        accounts.forEach { account ->
            val title = if (account.googleHostedPassword) "Sign in with Google"
                else if (accounts.size == 1) (if (tasks) "Allow Google Tasks access" else "Show your Google Calendar")
                else "${if (tasks) "Allow Google Tasks" else "Show calendar"} · ${account.email}"
            HouseButton(if (model.signingIn) "Connecting…" else title, if (tasks) R.drawable.ic_todo else R.drawable.ic_calendar, primary = true,
                enabled = !model.signingIn && !opening) {
                opening = true
                planner.perform {
                    val core = requireNotNull(planner.core)
                    val start = if (account.googleHostedPassword) core.beginSignIn(SignInProvider.GOOGLE) else core.beginCalendarAccess(account.id)
                    openSignIn(context, start.url)
                }
            }
        }
    }
}

private val HourHeight = 60.dp
/** The hour column: 48 dp, or wider when the locale's clock needs it ("12:00 PM" in a large font). */
private val Gutter: Dp @Composable get() = maxOf(48.dp, timeColumn() + 8.dp)

/** iPad's week, desktop `week::week_view`: day columns on an hour grid, all-day events above, the now line across. */
@Composable private fun WeekGrid(planner: PlannerModel, model: MailModel, oneDay: Boolean) {
    var selected by remember(planner.weekStart) { mutableStateOf(calendarDay(planner.weekStart, LocalDate.now())) }
    val week = (0L until 7L).map { planner.weekStart.plusDays(it) }
    val days = if (oneDay) listOf(selected) else week
    val byDay = planner.week.groupBy { it.day }
    var shown by remember { mutableStateOf<AgendaItem?>(null) }
    var creating by rememberSaveable { mutableStateOf<LocalDate?>(null) }
    // 16 dp as Today and To-do, on phones and tablets alike.
    Column(Modifier.fillMaxSize().padding(horizontal = 16.dp)) {
        WeekHeader(planner, if (oneDay) selected else planner.weekStart.plusDays(3), compact = oneDay,
            newEvent = {
                planner.eventAnswer = null
                creating = if (oneDay) selected else calendarDay(planner.weekStart, LocalDate.now())
            }.takeIf { planner.eventCalendars.isNotEmpty() }) {
            selected = LocalDate.now()
            planner.perform { planner.showWeek(0) }
        }
        if (oneDay) WeekStrip(week, selected) { selected = it }
        val access = planner.calendarAccess.firstOrNull()
        if (!planner.calendarConnected && planner.week.isEmpty() && access != null) {
            EmptyState(R.drawable.ic_calendar, "Calendar is not connected", Modifier.fillMaxSize(),
                if (planner.calendarAccess.size == 1) "Google asks once for access to ${access.email}." else "Google asks once for each account.") {
                CalendarAccessButtons(planner, model, planner.calendarAccess)
            }
        } else if (!planner.calendarConnected && planner.week.isEmpty() && planner.calendarGranted) {
            EmptyState(R.drawable.ic_calendar, "Calendar not synced yet", Modifier.fillMaxSize(), "Your calendar shows here after the next sync.")
        } else if (!planner.calendarConnected && planner.week.isEmpty()) {
            val upgrade = model.accounts.firstOrNull { it.googleHostedPassword }
            EmptyState(R.drawable.ic_calendar, "Calendar needs a Google account", Modifier.fillMaxSize(),
                if (upgrade == null) "Add a Google account to see your calendar here." else "Sign in with the same address to keep your mail.") {
                if (upgrade != null) CalendarAccessButtons(planner, model, listOf(upgrade))
            }
        } else {
            CalendarAccessButtons(planner, model, planner.calendarAccess, Modifier.padding(top = 8.dp))
            if (!oneDay) Row(Modifier.padding(top = 8.dp)) {
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
    creating?.let { NewEventDialog(planner, it) { creating = null } }
}

/** The selected day's month on phones, the week's Thursday on tablets, and the week buttons. */
@Composable private fun WeekHeader(planner: PlannerModel, month: LocalDate, compact: Boolean, newEvent: (() -> Unit)?, onToday: () -> Unit) {
    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Row(Modifier.weight(1f).semantics(mergeDescendants = true) { heading() }, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            val (name, year) = DateWords().monthTitle(month)
            Text(name, Modifier.weight(1f, fill = false), maxLines = 1, overflow = TextOverflow.Ellipsis,
                style = MaterialTheme.typography.titleLarge.copy(fontFamily = DisplayFace, fontWeight = FontWeight.Bold))
            year?.let { Text(it, maxLines = 1, style = MaterialTheme.typography.titleLarge, color = MaterialTheme.colorScheme.onSurfaceVariant) }
        }
        LagomIconButton("Previous week", R.drawable.ic_chevron_left) { planner.perform { planner.showWeek(-1) } }
        HouseButton("Today", onClick = onToday)
        LagomIconButton("Next week", R.drawable.ic_chevron_right) { planner.perform { planner.showWeek(1) } }
        // A phone's header holds the month and the week buttons on one line, so New event shows its icon there.
        newEvent?.let { HouseButton("New event", R.drawable.ic_new_event, primary = true, iconOnly = compact, onClick = it) }
    }
}

/** Desktop `new_event_sheet`: the same fields, words and defaults, with the system's date and time pickers. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable private fun NewEventDialog(planner: PlannerModel, day: LocalDate, close: () -> Unit) {
    val defaults = remember(day) { newEventTimes(day, LocalDateTime.now()) }
    var title by rememberSaveable { mutableStateOf("") }
    var start by rememberSaveable { mutableStateOf(defaults.first) }
    var end by rememberSaveable { mutableStateOf(defaults.second) }
    var allDay by rememberSaveable { mutableStateOf(false) }
    var meet by rememberSaveable { mutableStateOf(false) }
    var guests by rememberSaveable { mutableStateOf("") }
    var description by rememberSaveable { mutableStateOf("") }
    // The (account, calendar) shown when the dialog opened, or the one picked since: Save must not move the event to
    // whichever calendar a sync put first.
    var calendar by rememberSaveable { mutableStateOf(planner.eventCalendars.firstOrNull()?.let { it.account to it.calendar }) }
    var error by rememberSaveable { mutableStateOf<String?>(null) }
    // The model's, so a dialog restored after rotation stays busy until the first save answers.
    val saving = planner.savingEvent
    LaunchedEffect(planner.eventAnswer) {
        val answer = planner.eventAnswer ?: return@LaunchedEffect
        planner.eventAnswer = null
        if (answer.isEmpty()) close() else error = answer
    }
    // The open picker: "date", "start", "endDate" or "end".
    var picking by remember { mutableStateOf<String?>(null) }
    val calendars = planner.eventCalendars
    val focus = remember { FocusRequester() }
    // Moving the start keeps the length, as the system calendars do.
    fun moveStart(to: LocalDateTime) { end = end.plus(java.time.Duration.between(start, to)); start = to }
    val chosen = calendar
    // A calendar that left the list asks for a choice instead of picking another one.
    val gone = calendars.none { it.account to it.calendar == chosen }
    fun save() {
        if (saving) return
        val target = calendars.firstOrNull { it.account to it.calendar == chosen }
        if (target == null) { error = "Choose a calendar"; return }
        if (!allDay && !(unambiguous(start) && unambiguous(end))) {
            error = "This local time is skipped or repeated by daylight saving; choose an unambiguous time"
            return
        }
        error = null
        planner.saveEvent(target, eventForm(title, start, end, allDay, guests, description, meet))
    }
    val words = { date: LocalDate -> localDate(date, if (date.year == LocalDate.now().year) "EEEdMMM" else "EEEdMMMyyyy") }
    val clockFormat = android.text.format.DateFormat.getTimeFormat(LocalContext.current)
    val time = { at: LocalDateTime -> clockFormat.format(Date.from(at.atZone(ZoneId.systemDefault()).toInstant())) }
    AlertDialog(onDismissRequest = { if (!saving) close() }, title = { Text("New event") }, text = {
        Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            OutlinedTextField(title, { title = it }, label = { Text("Title") }, placeholder = { Text("Event title") }, singleLine = true, enabled = !saving,
                modifier = Modifier.fillMaxWidth().focusRequester(focus))
            PickerRow("Date", words(start.toLocalDate()), !saving) { picking = "date" }
            if (!allDay) PickerRow("Start", time(start), !saving) { picking = "start" }
            PickerRow(if (allDay) "Last day" else "End date", words(end.toLocalDate()), !saving) { picking = "endDate" }
            if (!allDay) PickerRow("End", time(end), !saving) { picking = "end" }
            SwitchRow("All day", R.drawable.ic_all_day, allDay, !saving) { allDay = it }
            if (calendars.size > 1 || gone) calendars.forEach { option ->
                val key = option.account to option.calendar
                Text(option.label, Modifier.fillMaxWidth().heightIn(min = 48.dp).clip(RoundedCornerShape(8.dp))
                    .background(if (key == chosen) MaterialTheme.colorScheme.secondaryContainer else Color.Transparent)
                    .selectable(key == chosen, enabled = !saving, role = Role.RadioButton) { calendar = key }
                    .wrapContentHeight().padding(horizontal = 8.dp), maxLines = 1, overflow = TextOverflow.Ellipsis)
            } else calendars.firstOrNull()?.let { Text(it.label, color = lagom.ink2, style = timeStyle()) }
            SwitchRow("Add Google Meet", R.drawable.ic_camera, meet, !saving) { meet = it }
            OutlinedTextField(guests, { guests = it }, label = { Text("Guests · email addresses (optional)") }, singleLine = true, enabled = !saving,
                placeholder = { Text("guest@example.com, guest2@example.com") },
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Email), modifier = Modifier.fillMaxWidth())
            OutlinedTextField(description, { description = it }, label = { Text("Description") }, placeholder = { Text("Description (optional)") },
                enabled = !saving, minLines = 2, modifier = Modifier.fillMaxWidth())
            if (planner.demo && meet) Text("Demo: no meeting link or invitations will be created", color = lagom.ink2, style = timeStyle())
            error?.let { Text(it, color = lagom.danger, style = MaterialTheme.typography.bodySmall) }
        }
    }, confirmButton = { TextButton(enabled = !saving, onClick = ::save) { Text(if (saving) "Saving…" else "Save event") } },
        dismissButton = { TextButton(enabled = !saving, onClick = close) { Text("Cancel") } })
    LaunchedEffect(Unit) { focus.requestFocus() }
    when (val which = picking) {
        "date", "endDate" -> {
            val current = if (which == "date") start else end
            // The picker counts in UTC midnights.
            val state = rememberDatePickerState(initialSelectedDateMillis = current.toLocalDate().atStartOfDay(ZoneOffset.UTC).toInstant().toEpochMilli())
            DatePickerDialog(onDismissRequest = { picking = null }, confirmButton = {
                TextButton(onClick = {
                    state.selectedDateMillis?.let { Instant.ofEpochMilli(it).atZone(ZoneOffset.UTC).toLocalDate() }?.let { date ->
                        if (which == "date") moveStart(date.atTime(start.toLocalTime())) else end = date.atTime(end.toLocalTime())
                    }
                    picking = null
                }) { Text("OK") }
            }, dismissButton = { TextButton(onClick = { picking = null }) { Text("Cancel") } }) { DatePicker(state) }
        }
        "start", "end" -> {
            val current = if (which == "start") start else end
            val state = rememberTimePickerState(current.hour, current.minute, android.text.format.DateFormat.is24HourFormat(LocalContext.current))
            AlertDialog(onDismissRequest = { picking = null }, text = { TimePicker(state) }, confirmButton = {
                TextButton(onClick = {
                    val at = current.toLocalDate().atTime(state.hour, state.minute)
                    if (which == "start") moveStart(at) else end = at
                    picking = null
                }) { Text("OK") }
            }, dismissButton = { TextButton(onClick = { picking = null }) { Text("Cancel") } })
        }
    }
}

/** A quiet label and the value; a tap opens its picker. */
@Composable private fun PickerRow(label: String, value: String, enabled: Boolean, open: () -> Unit) =
    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(label, Modifier.weight(1f), color = lagom.ink2)
        TextButton(enabled = enabled, onClick = open, modifier = Modifier.semantics { contentDescription = "$label, $value" }) { Text(value) }
    }

/** The Mac's toggle row: its icon, the label and a switch; the whole row toggles. */
@Composable private fun SwitchRow(label: String, icon: Int, on: Boolean, enabled: Boolean, change: (Boolean) -> Unit) =
    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).toggleable(on, enabled = enabled, role = Role.Switch, onValueChange = change),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        LagomIcon(icon, tint = lagom.ink2, size = 16.dp)
        Text(label, Modifier.weight(1f))
        Switch(on, onCheckedChange = null, enabled = enabled)
    }


/** Seven tappable days: the selection is filled, today otherwise stays in the accent. */
@Composable private fun WeekStrip(week: List<LocalDate>, selected: LocalDate, onSelect: (LocalDate) -> Unit) {
    val colors = MaterialTheme.colorScheme
    val today = LocalDate.now()
    Row(Modifier.fillMaxWidth().padding(top = 8.dp)) {
        week.forEach { day ->
            val chosen = day == selected
            Column(Modifier.weight(1f).heightIn(min = 48.dp)
                .selectable(chosen, role = Role.Button) { onSelect(day) }
                .semantics { contentDescription = localDate(day, "EEEEdMMMM") },
                horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text(localDate(day, "EEEEE"), Modifier.clearAndSetSemantics {}, color = colors.onSurfaceVariant, style = MaterialTheme.typography.labelSmall)
                Box(Modifier.size(40.dp).background(if (chosen) colors.primary else Color.Transparent, CircleShape).clearAndSetSemantics {},
                    contentAlignment = Alignment.Center) {
                    Text(day.dayOfMonth.toString(), style = MaterialTheme.typography.titleMedium.copy(fontFeatureSettings = "tnum"),
                        color = if (chosen) colors.onPrimary else if (day == today) colors.primary else colors.onSurface)
                }
            }
        }
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

/** Opens at the first event, the current hour on an empty today, or otherwise 08:00. */
@Composable private fun HourGrid(days: List<LocalDate>, byDay: Map<String, List<AgendaItem>>, onShow: (AgendaItem) -> Unit) {
    val blocks = days.map { weekBlocks(byDay[it.toString()].orEmpty(), it) }
    val first = blocks.flatten().minOfOrNull { it.start }?.let { maxOf(0, (it / 60).toInt() - 1) }
        ?: if (days.singleOrNull() == LocalDate.now()) LocalTime.now().hour else 8
    val scroll = rememberScrollState()
    val hour = with(LocalDensity.current) { HourHeight.roundToPx() }
    LaunchedEffect(days, first) { scroll.scrollTo(first * hour) }
    val colors = MaterialTheme.colorScheme
    val context = LocalContext.current
    val hours = remember(context) {
        val format = android.text.format.DateFormat.getTimeFormat(context)
        val at = java.util.Calendar.getInstance().apply { set(2026, 0, 1, 0, 0) } // a fixed date, as `timeColumn`
        (0..23).map { at.set(java.util.Calendar.HOUR_OF_DAY, it); format.format(at.time) }
    }
    Box(Modifier.fillMaxSize().verticalScroll(scroll).padding(top = 12.dp)) {
        Row {
            Column(Modifier.width(Gutter).clearAndSetSemantics {}) {
                (0 until 24).forEach { h ->
                    // One plain label in the locale's clock, "13:00" or "1:00 PM", as the Mac's gutter.
                    Text(hours[h], Modifier.height(HourHeight).fillMaxWidth().offset(y = (-8).dp).padding(end = 8.dp), color = colors.onSurfaceVariant,
                        style = timeStyle(), textAlign = TextAlign.End, maxLines = 1, softWrap = false)
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
