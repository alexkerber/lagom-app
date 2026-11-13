package app.lagommail.android

import org.junit.Assert.*
import org.junit.Test

class MotionTest {
    @Test fun fewChangesAnimateAndAPageAppearsStill() {
        assertTrue(smallChange(listOf(1, 2, 3), listOf(9, 1, 2, 3)))
        assertTrue(smallChange(listOf(1, 2, 3), listOf(1, 3)))
        assertTrue(smallChange(listOf(1, 2, 3), listOf(3, 2, 1)))
        assertTrue(smallChange(emptyList(), (1L..5L).toList()))
        assertFalse(smallChange(emptyList(), (1L..6L).toList()))
        assertFalse(smallChange((1L..100L).toList(), emptyList()))
        assertFalse(smallChange((1L..3L).toList(), (4L..6L).toList()))
    }
}
