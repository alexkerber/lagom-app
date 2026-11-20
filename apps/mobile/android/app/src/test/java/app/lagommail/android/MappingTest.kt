package app.lagommail.android

import app.lagommail.core.*
import org.junit.Assert.*
import org.junit.Test

internal fun row(id: Long, account: String = "fixture", unread: Boolean = false) = MailRow(
    id, account, "", "sender@example.test", "", "Preview", 1_700_000_000, unread, true, "thread-$id", 1u, listOf(id))

class MappingTest {
    @Test fun rowMappingPreservesFlagsDateAndFallbacks() {
        val value = RowPresentation(row(42, unread = true))
        assertEquals("sender@example.test", value.sender)
        assertEquals("(No subject)", value.subject)
        assertEquals(1_700_000_000L, value.date.epochSecond)
        assertEquals("Preview", value.row.preview)
        assertTrue(value.row.unread)
        assertTrue(value.row.hasAttachments)
        assertEquals("Sender", RowPresentation(row(1).copy(senderName = "Sender")).sender)
    }
    @Test fun replyMappingKeepsAccountCcAndThread() {
        val reply = ReplyDraft("second-account", "sender@example.test", "copy@example.test", "Re: Subject", "\n\n> Quote", 42, emptyList())
        val draft = ComposeDraft(reply)
        assertEquals(reply.account, draft.account)
        assertEquals(reply.to, draft.to)
        assertEquals(reply.subject, draft.subject)
        assertEquals(reply.body, draft.body)
        assertEquals(reply.replyTo, draft.replyTo)
        assertEquals(reply.cc, draft.cc)
        assertNull(ComposeDraft(reply.copy(replyTo = null)).replyTo)
    }
    @Test fun everyMailboxHasItsLabel() {
        assertEquals(listOf("Inbox", "Later", "Done", "Sent", "Trash"), Mailbox.entries.map { it.label() })
    }
    @Test fun mailboxesFollowTheDesktopOrder() {
        assertEquals(listOf("Inbox", "Later", "Sent", "Done", "Drafts", "Trash"), MailboxOrder.map { it?.label() ?: "Drafts" })
    }
    @Test fun mailDatesUseRelativeWordsAsDesktopAndIos() {
        // Tuesday 10 November 2026, as iOS PlannerTests.
        val zone = java.time.ZoneOffset.UTC
        val dates = MailDates({ "clock" }, java.time.LocalDate.of(2026, 11, 10), zone) { date, skeleton -> "$skeleton $date" }
        fun at(day: String) = java.time.LocalDate.parse(day).atTime(9, 42).toInstant(zone)
        assertEquals("clock", dates.listTime(at("2026-11-10")))
        assertEquals("Yesterday", dates.listTime(at("2026-11-09")))
        assertEquals("EEE 2026-11-06", dates.listTime(at("2026-11-06")))
        assertEquals("dMMM 2026-10-26", dates.listTime(at("2026-10-26")))
        assertEquals("MMMyyyy 2025-10-01", dates.listTime(at("2025-10-01")))
        assertEquals("Today clock", dates.readingDate(at("2026-11-10")))
        assertEquals("Yesterday clock", dates.readingDate(at("2026-11-09")))
        assertEquals("EEE 2026-11-06 clock", dates.readingDate(at("2026-11-06")))
        assertEquals("dMMM 2026-10-26, clock", dates.readingDate(at("2026-10-26")))
        assertEquals("dMMMyyyy 2025-10-01, clock", dates.readingDate(at("2025-10-01")))
        // A skewed sender clock: the future reads as a date, and groups under Today.
        assertEquals("dMMM 2026-11-12, clock", dates.readingDate(at("2026-11-12")))
        assertEquals("Today", dates.group(at("2026-11-12")))
        assertEquals("Today", dates.group(at("2026-11-10")))
        assertEquals("Yesterday", dates.group(at("2026-11-09")))
        // Monday is the only earlier day of this week; Sunday belongs to last week.
        assertEquals("Earlier", dates.group(at("2026-11-08")))
        val friday = MailDates({ "clock" }, java.time.LocalDate.of(2026, 11, 13), zone) { date, skeleton -> "$skeleton $date" }
        assertEquals("EEEE 2026-11-11", friday.group(at("2026-11-11")))
        assertEquals("EEEE 2026-11-09", friday.group(at("2026-11-09")))
        assertEquals("Earlier", friday.group(at("2026-11-08")))
    }
    @Test fun groupsAreRunsInListOrder() {
        assertEquals(listOf("a" to listOf(1, 2), "b" to listOf(3), "a" to listOf(4)), grouped(listOf(1, 2, 3, 4)) { if (it == 3) "b" else "a" })
        assertEquals(emptyList<Pair<String, List<Int>>>(), grouped(emptyList<Int>()) { "a" })
    }
    @Test fun manualServerValidatesPortsAndMapsTransport() {
        assertEquals(993.toUShort(), server(" mail.example.test ", "993", " user ", true).port)
        assertEquals(TransportSecurity.TLS, server("host", "993", "user", true).security)
        assertEquals(TransportSecurity.START_TLS, server("host", "587", "user", false).security)
        for (port in listOf("0", "65536", "-1", "abc", "")) {
            assertThrows(IllegalArgumentException::class.java) { server("host", port, "user", true) }
        }
    }
    @Test fun coreErrorsShowTheirReadableReason() {
        assertEquals("Fixture refusal", MailException.Failed("Fixture refusal").mailMessage())
    }
    @Test fun browserLinksOnlyUseAllowedSchemes() {
        for (link in listOf("https://example.test", "HTTP://example.test", "mailto:to@example.test")) assertTrue(externalMailLink(link))
        for (link in listOf("file:///etc/passwd", "content://mail", "intent://x", "javascript:alert(1)", "data:text/html,x", "/relative", "bad uri")) assertFalse(externalMailLink(link))
    }
    @Test fun mailtoAndSharesMapToANewDraftWithoutBcc() {
        val link = Mailto(listOf("a@example.test"), listOf("c@example.test"), listOf("hidden@example.test"), "Hi", "Line 1\nLine 2")
        val parsed = mutableListOf<String>()
        val parse = { url: String -> parsed += url; if (url.startsWith("mailto:")) link else null }
        val view = sharedMail("android.intent.action.VIEW", "mailto:a@example.test", emptyList(), emptyList(), emptyList(), "Ignored", "Ignored", parse)!!
        assertEquals(listOf("mailto:a@example.test"), parsed)
        assertEquals(link, view)
        val sendTo = sharedMail("android.intent.action.SENDTO", "mailto:", listOf("b@example.test"), emptyList(), emptyList(), null, null) {
            Mailto(emptyList(), emptyList(), emptyList(), "", "") }!!
        assertEquals(listOf("b@example.test"), sendTo.to)
        assertNull(sharedMail("android.intent.action.VIEW", "https://example.test", emptyList(), emptyList(), emptyList(), null, null, parse))
        assertNull(sharedMail("android.intent.action.VIEW", null, emptyList(), emptyList(), emptyList(), null, null, parse))
        assertNull(sharedMail("android.intent.action.MAIN", null, emptyList(), emptyList(), emptyList(), null, null, parse))
        val share = sharedMail("android.intent.action.SEND", null, listOf("to@example.test\r\nBcc: x@example.test", " "), listOf("cc@example.test"),
            listOf("bcc@example.test"), "Subject\r\nBcc: x@example.test", "Text\r\nmore") { error("SEND has no link") }!!
        assertEquals(listOf("to@example.test  Bcc: x@example.test"), share.to)
        assertEquals("Subject  Bcc: x@example.test", share.subject)
        assertEquals("Text\nmore", share.body)
        assertEquals(listOf("bcc@example.test"), share.bcc)
        val file = SendAttachment("a.txt", "text/plain", byteArrayOf(1))
        val draft = sharedDraft("account", "me@example.test", SharedMail(link, listOf(file)))
        assertEquals("account", draft.account)
        assertEquals("a@example.test", draft.to); assertEquals("c@example.test", draft.cc)
        assertEquals("Hi", draft.subject); assertEquals("Line 1\nLine 2", draft.body)
        assertEquals(listOf(file), draft.attachments)
        assertTrue(draft.id.endsWith("@example.test>")); assertNull(draft.replyTo); assertEquals("New message", draft.title)
        assertFalse(listOf(draft.to, draft.cc, draft.subject, draft.body).any { "hidden@example.test" in it })
        // A link's Bcc shows in its own field; the signature goes under the link's body.
        assertEquals("hidden@example.test", draft.bcc); assertTrue(draft.showBcc)
        assertEquals("Line 1\nLine 2\n\n-- \nTeam", sharedDraft("account", "me@example.test", SharedMail(link), "\n\n-- \nTeam").body)
        assertFalse(ComposeDraft("account").showBcc)
        val empty = SharedMail(Mailto(emptyList(), emptyList(), emptyList(), "", ""))
        assertFalse("a link's signature alone is no draft", sharedDraft("account", "me@example.test", empty, "\n\n-- \nTeam").hasContent)
        assertTrue(ComposeDraft(LocalDraft("id", "account", "", "", "", "", null, emptyList(), emptyList(), "b@example.test")).showBcc)
    }
}
