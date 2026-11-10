package app.lagommail.android

import org.junit.Assert.*
import org.junit.Test
import javax.crypto.AEADBadTagException
import javax.crypto.KeyGenerator

class SecretCipherTest {
    private val key = KeyGenerator.getInstance("AES").apply { init(256) }.generateKey()
    @Test fun roundTripUsesFreshIvAndDoesNotStorePlaintext() {
        val secret = "fixture-only password".toByteArray()
        val first = SecretCipher.encrypt(key, "fixture\u0000one", secret)
        val second = SecretCipher.encrypt(key, "fixture\u0000one", secret)
        assertFalse(first.contentEquals(second))
        assertFalse(String(first).contains(String(secret)))
        assertArrayEquals(secret, SecretCipher.decrypt(key, "fixture\u0000one", first))
    }
    @Test fun rejectsTamperingAndAccountSubstitution() {
        val stored = SecretCipher.encrypt(key, "fixture\u0000one", byteArrayOf(1, 2, 3))
        assertThrows(AEADBadTagException::class.java) { SecretCipher.decrypt(key, "fixture\u0000two", stored) }
        stored[stored.lastIndex] = (stored.last().toInt() xor 1).toByte()
        assertThrows(AEADBadTagException::class.java) { SecretCipher.decrypt(key, "fixture\u0000one", stored) }
    }
    @Test fun rejectsMissingKeyMalformedAndUnknownVersions() {
        val other = KeyGenerator.getInstance("AES").apply { init(256) }.generateKey()
        val stored = SecretCipher.encrypt(key, "fixture", byteArrayOf(1))
        assertThrows(AEADBadTagException::class.java) { SecretCipher.decrypt(other, "fixture", stored) }
        assertThrows(IllegalArgumentException::class.java) { SecretCipher.decrypt(key, "fixture", byteArrayOf()) }
        stored[0] = 2
        assertThrows(IllegalArgumentException::class.java) { SecretCipher.decrypt(key, "fixture", stored) }
    }
}
