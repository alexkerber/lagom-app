package app.lagommail.android

import org.junit.Assert.*
import org.junit.Test

class DebugLaunchTest {
    @Test fun releaseIgnoresDemoAndTheme() {
        assertEquals(DebugLaunch(), debugLaunch(debug = false, demo = "mail", theme = "dark"))
        assertEquals(DebugLaunch(), debugLaunch(debug = false, demo = "welcome", theme = "light"))
    }
    @Test fun debugTakesOnlyKnownDemos() {
        for (demo in listOf("mail", "welcome", "notifications", "calendar-google", "calendar-granted", "calendar-none")) assertEquals(demo, debugLaunch(true, demo, null).demo)
        assertNull(debugLaunch(true, "anything", null).demo)
        assertNull(debugLaunch(true, null, null).demo)
    }
    @Test fun debugThemeForcesLightOrDarkOtherwiseFollowsSystem() {
        assertEquals(true, debugLaunch(true, null, "dark").dark)
        assertEquals(false, debugLaunch(true, null, "light").dark)
        assertNull(debugLaunch(true, null, "blue").dark)
    }
}
