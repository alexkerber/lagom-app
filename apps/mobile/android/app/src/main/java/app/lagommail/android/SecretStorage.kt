package app.lagommail.android

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.AtomicFile
import app.lagommail.core.MailException
import app.lagommail.core.SecretStore
import java.io.File
import java.io.FileNotFoundException
import java.security.KeyStore
import java.security.MessageDigest
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

internal object SecretCipher {
    fun encrypt(key: SecretKey, identity: String, secret: ByteArray): ByteArray {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, key)
        cipher.updateAAD(identity.toByteArray(Charsets.UTF_8))
        return byteArrayOf(1) + cipher.iv + cipher.doFinal(secret)
    }

    fun decrypt(key: SecretKey, identity: String, stored: ByteArray): ByteArray {
        require(stored.size >= 29 && stored[0] == 1.toByte()) { "Invalid encrypted credential" }
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, stored.copyOfRange(1, 13)))
        cipher.updateAAD(identity.toByteArray(Charsets.UTF_8))
        return cipher.doFinal(stored, 13, stored.size - 13)
    }
}

internal class AndroidSecretStore(context: Context) : SecretStore {
    private val directory = File(context.noBackupFilesDir, "credentials").apply {
        check(mkdirs() || isDirectory) { "Secure storage unavailable" }
    }
    private val key: SecretKey by lazy {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey("lagom.credentials", null) as? SecretKey) ?: KeyGenerator.getInstance(
            KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore"
        ).apply {
            init(KeyGenParameterSpec.Builder("lagom.credentials", KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256).build())
        }.generateKey()
    }

    private fun identity(service: String, user: String): String = "$service\u0000$user"
    private fun file(identity: String): AtomicFile {
        val name = MessageDigest.getInstance("SHA-256").digest(identity.toByteArray(Charsets.UTF_8))
            .joinToString("") { "%02x".format(it) }
        return AtomicFile(File(directory, name))
    }
    private fun <T> secure(action: () -> T): T = try { action() } catch (_: Exception) {
        // Never let provider exceptions or credential bytes cross the error boundary.
        throw MailException.Failed("Secure storage unavailable; unlock the device and try again.")
    }

    private fun read(file: AtomicFile): ByteArray? = try { file.readFully() } catch (e: FileNotFoundException) {
        // openRead recovers interrupted writes before deciding whether an entry is missing.
        if (file.baseFile.exists()) throw e else null
    }

    @Synchronized override fun get(service: String, user: String): ByteArray? = secure {
        val identity = identity(service, user)
        val file = file(identity)
        read(file)?.let { SecretCipher.decrypt(key, identity, it) }
    }
    @Synchronized override fun set(service: String, user: String, secret: ByteArray) = secure {
        try {
            val identity = identity(service, user)
            val encrypted = SecretCipher.encrypt(key, identity, secret)
            val file = file(identity)
            val stream = file.startWrite()
            try { stream.write(encrypted); file.finishWrite(stream) }
            catch (error: Exception) { file.failWrite(stream); throw error }
        } finally { secret.fill(0) }
    }
    @Synchronized override fun delete(service: String, user: String) = secure {
        val file = file(identity(service, user))
        file.delete()
        check(read(file) == null) { "Credential deletion failed" }
    }
}
