package app.lagommail.android

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.*
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.lagommail.core.Mailbox
import app.lagommail.core.MailCounts

internal fun List<MailCounts>.sidebarCount(box: Mailbox, account: String? = null): ULong =
    filter { account == null || it.account == account }.sumOf {
        when (box) { Mailbox.INBOX -> it.inbox; Mailbox.LATER -> it.later; else -> 0uL }
    }

/** The iPad sidebar: pages, Mail's scopes, then accounts. */
@Composable internal fun LagomSidebar(model: MailModel, section: AppSection, unread: Int, enabled: Boolean, choose: (AppSection) -> Unit) {
    fun active(box: Mailbox, account: String? = null) =
        !model.showingDrafts && model.folder == null && model.mailbox == box && model.accountId == account
    Column(Modifier.width(240.dp).fillMaxHeight().padding(horizontal = 12.dp).padding(top = 8.dp)) {
        // No logo, as the iPad sidebar: the pages start at the top.
        Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Column(Modifier.selectableGroup(), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                AppSection.entries.forEach { page ->
                    SidebarRow(page.title, page.icon, active = section == page, page = true, enabled = enabled,
                        count = if (page == AppSection.MAIL && unread > 0) unread.toString() else null,
                        label = if (page == AppSection.MAIL) "Mail, $unread unread" else page.title) { choose(page) }
                }
            }
            if (section == AppSection.MAIL) {
                Spacer(Modifier.height(16.dp))
                SidebarRow("Inbox", R.drawable.ic_inbox, active = active(Mailbox.INBOX), enabled = enabled,
                    count = model.counts.sidebarCount(Mailbox.INBOX).takeIf { it > 0uL }?.toString()) {
                    model.perform { model.changeScope(null, Mailbox.INBOX) }
                }
                if (model.accounts.size > 1) model.accounts.forEach { account ->
                    SidebarRow(account.displayName, R.drawable.ic_inbox, Modifier.padding(start = 16.dp),
                        active = active(Mailbox.INBOX, account.id), enabled = enabled, label = "${account.displayName} Inbox",
                        count = model.counts.sidebarCount(Mailbox.INBOX, account.id).takeIf { it > 0uL }?.toString()) {
                        model.perform { model.changeScope(account.id, Mailbox.INBOX) }
                    }
                }
                SidebarRow("Later", R.drawable.ic_later, active = active(Mailbox.LATER), enabled = enabled,
                    count = model.counts.sidebarCount(Mailbox.LATER).takeIf { it > 0uL }?.toString()) {
                    model.perform { model.changeScope(null, Mailbox.LATER) }
                }
                // The rest of `MailboxOrder`: Sent, Done, Drafts, Trash.
                listOf(Mailbox.SENT to R.drawable.ic_sent, Mailbox.DONE to R.drawable.ic_done).forEach { (box, icon) ->
                    SidebarRow(box.label(), icon, active = active(box), enabled = enabled) { model.perform { model.changeScope(null, box) } }
                }
                SidebarRow("Drafts", R.drawable.ic_edit_draft, active = model.showingDrafts, enabled = enabled) {
                    model.perform { model.showDrafts() }
                }
                SidebarRow("Trash", R.drawable.ic_trash, active = active(Mailbox.TRASH), enabled = enabled) {
                    model.perform { model.changeScope(null, Mailbox.TRASH) }
                }
                if (model.accounts.isNotEmpty()) {
                    SidebarHeading("Folders")
                    model.accounts.forEach { account ->
                        if (model.accounts.size > 1) Text(account.displayName, Modifier.padding(horizontal = 10.dp),
                            color = lagom.ink2, style = MaterialTheme.typography.bodySmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        model.folders.filter { it.account == account.id }.forEach { folder ->
                            SidebarRow(folder.name, R.drawable.ic_folder, active = model.folder?.id == folder.id, enabled = enabled,
                                label = folder.path, count = folder.unread.takeIf { it > 0uL }?.toString()) {
                                model.perform { model.openFolder(folder) }
                            }
                        }
                        SidebarRow("New folder", R.drawable.ic_new_folder, enabled = enabled && !model.working,
                            label = "New folder in ${account.displayName}") { model.newFolder = NewFolder(account.id) }
                    }
                }
            }
            SidebarHeading("Accounts")
            model.accounts.forEach { account ->
                Text(account.email, Modifier.fillMaxWidth().heightIn(min = 48.dp).wrapContentHeight().padding(horizontal = 10.dp),
                    maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodyMedium)
            }
            SidebarRow("Add account", R.drawable.ic_account_add, enabled = enabled && model.core != null) {
                choose(AppSection.MAIL)
                model.accountForm = AccountForm()
            }
        }
    }
}

@Composable private fun SidebarHeading(title: String) {
    Text(title, Modifier.padding(top = 18.dp, start = 10.dp, bottom = 4.dp).semantics { heading() },
        color = lagom.ink2, style = MaterialTheme.typography.labelMedium)
}

@Composable private fun SidebarRow(title: String, icon: Int, modifier: Modifier = Modifier, active: Boolean = false,
    page: Boolean = false, enabled: Boolean = true, count: String? = null, label: String = title, action: () -> Unit) {
    val c = lagom
    Row(modifier.fillMaxWidth().heightIn(min = 48.dp).clip(RoundedCornerShape(Radius.control))
        .background(if (active && page) c.sel else Color.Transparent)
        .selectable(active, enabled = enabled, role = if (page) Role.Tab else Role.Button, onClick = action)
        .semantics { contentDescription = label }.padding(horizontal = 10.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        LagomIcon(icon, tint = if (active) { if (page) c.selIcon else c.accent } else c.ink2)
        Text(title, Modifier.weight(1f).clearAndSetSemantics {}, color = if (active && !page) c.accent else c.ink,
            style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
        if (count != null) Text(count, Modifier.clearAndSetSemantics {}, color = c.ink2, style = MaterialTheme.typography.bodySmall)
    }
}
