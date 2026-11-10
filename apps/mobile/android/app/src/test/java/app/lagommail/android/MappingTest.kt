package app.lagommail.android

import app.lagommail.core.*
import org.junit.Assert.*
import org.junit.Test

internal fun row(id: Long, account: String = "fixture", unread: Boolean = false) = MailRow(
    id, account, "", "sender@example.test", "", "Preview", 1_700_000_000, unread, true)

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
        assertEquals(listOf("Inbox", "Later", "Done", "Sent"), Mailbox.entries.map { it.label() })
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
}
