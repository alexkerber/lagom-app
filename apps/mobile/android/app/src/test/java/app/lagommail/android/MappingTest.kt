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
    }
}
