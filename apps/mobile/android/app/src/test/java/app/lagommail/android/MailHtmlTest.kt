package app.lagommail.android

import kotlinx.coroutines.*
import org.junit.Assert.*
import org.junit.Test
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

class MailHtmlTest {
    @Test fun largeDocumentIsPreparedOffTheCallerThread() = runBlocking {
        val caller = Thread.currentThread()
        val html = "x".repeat(4 * 1024 * 1024)
        val result = prepareMailDocument(html, true) { body, dark ->
            assertNotSame(caller, Thread.currentThread())
            assertSame(html, body)
            assertTrue(dark)
            "prepared"
        }
        assertEquals("prepared", result)
        assertSame(caller, Thread.currentThread())
    }

    @Test fun cancelledPreparationCannotPublishItsDocument() = runBlocking {
        val started = CountDownLatch(1)
        val release = CountDownLatch(1)
        var published: String? = null
        val job = launch {
            published = prepareMailDocument("old message", false) { _, _ ->
                started.countDown()
                check(release.await(5, TimeUnit.SECONDS))
                "old document"
            }
        }
        yield()
        try {
            assertTrue(started.await(5, TimeUnit.SECONDS))
            job.cancel()
        } finally { release.countDown() }
        job.join()
        assertNull(published)
    }
}
