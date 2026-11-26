package com.vbs.clipbridge

import java.security.MessageDigest
import java.security.SecureRandom
import javax.crypto.Cipher
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.SecretKeySpec

/**
 * Wire crypto, shared by the app, the IME and any third-party integration.
 * key = SHA-256("clipbridge:" + pin), frame = [12-byte nonce][AES-256-GCM ciphertext].
 */
object Crypto {
    fun key(pin: String): ByteArray =
        MessageDigest.getInstance("SHA-256")
            .digest(("clipbridge:" + pin).toByteArray(Charsets.UTF_8))

    fun encrypt(key: ByteArray, plain: ByteArray): ByteArray {
        val nonce = ByteArray(12).also { SecureRandom().nextBytes(it) }
        val c = Cipher.getInstance("AES/GCM/NoPadding")
        c.init(Cipher.ENCRYPT_MODE, SecretKeySpec(key, "AES"), GCMParameterSpec(128, nonce))
        return nonce + c.doFinal(plain)
    }

    fun decrypt(key: ByteArray, data: ByteArray): ByteArray? = try {
        if (data.size < 13) null else {
            val c = Cipher.getInstance("AES/GCM/NoPadding")
            c.init(
                Cipher.DECRYPT_MODE,
                SecretKeySpec(key, "AES"),
                GCMParameterSpec(128, data.copyOfRange(0, 12))
            )
            c.doFinal(data.copyOfRange(12, data.size))
        }
    } catch (_: Exception) {
        null
    }
}
