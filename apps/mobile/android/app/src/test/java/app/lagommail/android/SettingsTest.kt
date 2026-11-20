package app.lagommail.android

import app.lagommail.core.MailAccount
import org.junit.Assert.assertEquals
import org.junit.Test

class SettingsTest {
    @Test fun accountRowsSayNameThenKindAsOnTheMac() {
        assertEquals("Personal · Email and password", accountKind(MailAccount("a", "me@home.test", "Personal", "Email and password")))
        assertEquals("Google account", accountKind(MailAccount("a", "me@home.test", "me@home.test", "Google account")))
        assertEquals("Personal", accountKind(MailAccount("a", "me@home.test", "Personal")))
    }
}
