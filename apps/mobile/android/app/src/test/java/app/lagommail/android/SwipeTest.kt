package app.lagommail.android

import org.junit.Assert.*
import org.junit.Test

class SwipeTest {
    @Test fun rightThresholdSelectsDone() {
        assertNull(swipeStage(119f, 400f, 120f, true))
        assertEquals(MailAction.Done, swipeStage(120f, 400f, 120f, true))
        assertEquals(MailAction.Done, swipeStage(400f, 400f, 120f, true))
    }

    @Test fun leftStagesFollowDistanceInBothDirections() {
        assertNull(swipeStage(-119f, 400f, 120f, true))
        assertEquals(MailAction.Later, swipeStage(-120f, 400f, 120f, true))
        assertEquals(MailAction.Later, swipeStage(-239f, 400f, 120f, true))
        assertEquals(MailAction.Delete, swipeStage(-240f, 400f, 120f, true))
        assertEquals(MailAction.Later, swipeStage(-200f, 400f, 120f, true))
        assertNull(swipeStage(-50f, 400f, 120f, true))
        assertNull(swipeStage(0f, 400f, 120f, true))
    }

    @Test fun deleteLineScalesWithRowWidth() {
        assertEquals(MailAction.Later, swipeStage(-359f, 600f, 120f, true))
        assertEquals(MailAction.Delete, swipeStage(-360f, 600f, 120f, true))
        assertNull(swipeStage(-59f, 100f, 120f, true))
        assertEquals(MailAction.Delete, swipeStage(-60f, 100f, 120f, true))
    }

    @Test fun trashStopsAtLater() {
        assertEquals(MailAction.Later, swipeStage(-120f, 400f, 120f, false))
        assertEquals(MailAction.Later, swipeStage(-240f, 400f, 120f, false))
        assertEquals(MailAction.Later, swipeStage(-400f, 400f, 120f, false))
        assertEquals(MailAction.Done, swipeStage(120f, 400f, 120f, false))
        assertNull(swipeStage(0f, 400f, 120f, false))
    }
}
