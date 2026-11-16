package app.lagommail.android

import app.lagommail.core.Mailbox
import app.lagommail.core.MailCounts
import org.junit.Assert.assertEquals
import org.junit.Test

class SidebarTest {
    @Test fun countsFollowUnifiedAndAccountScopes() {
        val counts = listOf(MailCounts("a", 3uL, 2uL), MailCounts("b", 5uL, 7uL))
        assertEquals(8uL, counts.sidebarCount(Mailbox.INBOX))
        assertEquals(3uL, counts.sidebarCount(Mailbox.INBOX, "a"))
        assertEquals(9uL, counts.sidebarCount(Mailbox.LATER))
        assertEquals(0uL, counts.sidebarCount(Mailbox.INBOX, "missing"))
        assertEquals(0uL, counts.sidebarCount(Mailbox.SENT))
        assertEquals(0uL, emptyList<MailCounts>().sidebarCount(Mailbox.INBOX))
    }
}
