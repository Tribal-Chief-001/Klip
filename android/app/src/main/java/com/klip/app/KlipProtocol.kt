package com.klip.app

import org.json.JSONArray
import org.json.JSONObject
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.security.MessageDigest
import javax.crypto.Cipher
import javax.crypto.spec.IvParameterSpec
import javax.crypto.spec.SecretKeySpec

object KlipProtocol {
    val MAGIC_BYTES = byteArrayOf('K'.code.toByte(), 'L'.code.toByte(), 'I'.code.toByte(), 'P'.code.toByte())
    const val CURRENT_VERSION: Byte = 1
    const val HEADER_LEN: Int = 72
    const val TAG_LEN: Int = 16
    const val MAX_PAYLOAD_SIZE: Int = 64 * 1024 * 1024

    // Message Types
    const val MSG_HANDSHAKE: Byte = 0x01
    const val MSG_HANDSHAKE_ACK: Byte = 0x02
    const val MSG_CLIP_FULL_PAYLOAD: Byte = 0x10
    const val MSG_CLIP_DELTA_PATCH: Byte = 0x11
    const val MSG_HEARTBEAT_PING: Byte = 0xF0.toByte()
    const val MSG_HEARTBEAT_PONG: Byte = 0xF1.toByte()

    // Flags
    const val FLAG_ENCRYPTED: Short = 0x0001
    const val FLAG_DELTA: Short = 0x0002
    const val FLAG_SENSITIVE: Short = 0x0004
    const val FLAG_COMPRESSED: Short = 0x0008

    data class WireHeader(
        val magic: ByteArray,         // 4 bytes
        val version: Byte,            // 1 byte
        val msgType: Byte,            // 1 byte
        val flags: Short,             // 2 bytes BE
        val senderNodeId: ByteArray,  // 32 bytes
        val nonce: ByteArray,         // 12 bytes
        val hlcPhysical: Long,        // 8 bytes BE
        val hlcLogical: Int,          // 4 bytes BE
        val hlcNodeSeed: Int,         // 4 bytes BE
        val payloadLen: Int           // 4 bytes BE
    ) {
        fun toBytes(): ByteArray {
            val buf = ByteBuffer.allocate(HEADER_LEN).order(ByteOrder.BIG_ENDIAN)
            buf.put(magic)
            buf.put(version)
            buf.put(msgType)
            buf.putShort(flags)
            buf.put(senderNodeId)
            buf.put(nonce)
            buf.putLong(hlcPhysical)
            buf.putInt(hlcLogical)
            buf.putInt(hlcNodeSeed)
            buf.putInt(payloadLen)
            return buf.array()
        }

        companion object {
            fun parse(bytes: ByteArray): WireHeader {
                require(bytes.size >= HEADER_LEN) { "Header buffer too short: ${bytes.size}" }
                val buf = ByteBuffer.wrap(bytes).order(ByteOrder.BIG_ENDIAN)
                val magic = ByteArray(4)
                buf.get(magic)
                require(magic.contentEquals(MAGIC_BYTES)) { "Invalid magic header" }
                val version = buf.get()
                require(version == CURRENT_VERSION) { "Unsupported version: $version" }
                val msgType = buf.get()
                val flags = buf.short
                val senderNodeId = ByteArray(32)
                buf.get(senderNodeId)
                val nonce = ByteArray(12)
                buf.get(nonce)
                val hlcPhysical = buf.long
                val hlcLogical = buf.int
                val hlcNodeSeed = buf.int
                val payloadLen = buf.int
                return WireHeader(
                    magic, version, msgType, flags,
                    senderNodeId, nonce, hlcPhysical, hlcLogical, hlcNodeSeed, payloadLen
                )
            }
        }
    }

    data class WirePacket(
        val header: WireHeader,
        val payload: ByteArray,
        val tag: ByteArray
    ) {
        fun encode(): ByteArray {
            val headerBytes = header.toBytes()
            val total = HEADER_LEN + payload.size + TAG_LEN
            val out = ByteArray(total)
            System.arraycopy(headerBytes, 0, out, 0, HEADER_LEN)
            System.arraycopy(payload, 0, out, HEADER_LEN, payload.size)
            System.arraycopy(tag, 0, out, HEADER_LEN + payload.size, TAG_LEN)
            return out
        }

        companion object {
            fun decode(bytes: ByteArray): WirePacket {
                require(bytes.size >= HEADER_LEN + TAG_LEN) { "Packet too short: ${bytes.size}" }
                val header = WireHeader.parse(bytes)
                val pLen = header.payloadLen
                val expectedTotal = HEADER_LEN + pLen + TAG_LEN
                require(bytes.size >= expectedTotal) { "Packet truncated: ${bytes.size} < $expectedTotal" }
                val payload = ByteArray(pLen)
                System.arraycopy(bytes, HEADER_LEN, payload, 0, pLen)
                val tag = ByteArray(TAG_LEN)
                System.arraycopy(bytes, HEADER_LEN + pLen, tag, 0, TAG_LEN)
                return WirePacket(header, payload, tag)
            }
        }
    }

    data class PairingInvitation(
        val hostNodeId: ByteArray,
        val psk: ByteArray,
        val timestamp: Long,
        val endpoints: List<String>
    )

    fun parsePairingToken(rawInput: String): PairingInvitation {
        val trimmed = rawInput.trim()

        // 1. If hex-encoded JSON
        val jsonStr = try {
            if (trimmed.startsWith("{") && trimmed.endsWith("}")) {
                trimmed
            } else if (trimmed.matches(Regex("^[0-9a-fA-F]+$"))) {
                String(hexToBytes(trimmed), Charsets.UTF_8)
            } else {
                trimmed
            }
        } catch (_: Exception) {
            trimmed
        }

        val json = JSONObject(jsonStr)
        val hostNodeArr = json.getJSONArray("host_node")
        val hostNode = ByteArray(32)
        for (i in 0 until 32) {
            hostNode[i] = hostNodeArr.getInt(i).toByte()
        }

        val pskHex = json.getString("psk_hex")
        val psk = hexToBytes(pskHex)
        require(psk.size == 32) { "PSK must be 32 bytes, got ${psk.size}" }

        val timestamp = json.optLong("timestamp", System.currentTimeMillis() / 1000)

        val endpoints = mutableListOf<String>()
        val endpointsArr = json.optJSONArray("endpoints")
        if (endpointsArr != null) {
            for (i in 0 until endpointsArr.length()) {
                endpoints.add(endpointsArr.getString(i))
            }
        }

        return PairingInvitation(hostNode, psk, timestamp, endpoints)
    }

    fun makeNonce(counter: Long): ByteArray {
        val nonce = ByteArray(12)
        val buf = ByteBuffer.wrap(nonce, 4, 8).order(ByteOrder.BIG_ENDIAN)
        buf.putLong(counter)
        return nonce
    }

    private fun getChaChaCipher(): Cipher {
        val transforms = listOf(
            "ChaCha20-Poly1305",
            "ChaCha20/Poly1305/NoPadding",
            "ChaCha20-Poly1305/None/NoPadding"
        )
        for (tf in transforms) {
            try {
                return Cipher.getInstance(tf)
            } catch (_: Throwable) {}
        }
        return Cipher.getInstance("ChaCha20-Poly1305")
    }

    private fun createKeySpec(key: ByteArray): SecretKeySpec {
        return try {
            SecretKeySpec(key, "ChaCha20")
        } catch (_: Throwable) {
            SecretKeySpec(key, "ChaCha20-Poly1305")
        }
    }

    /**
     * ChaCha20-Poly1305 AEAD detached encryption.
     * Compatible with Conscrypt (Android) and SunJCE (JVM).
     */
    fun encryptWithAad(
        key: ByteArray,
        nonce: ByteArray,
        aad: ByteArray,
        plaintext: ByteArray
    ): Pair<ByteArray, ByteArray> {
        val cipher = getChaChaCipher()
        val keySpec = createKeySpec(key)
        val paramSpec = IvParameterSpec(nonce)
        cipher.init(Cipher.ENCRYPT_MODE, keySpec, paramSpec)
        if (aad.isNotEmpty()) {
            cipher.updateAAD(aad)
        }
        val combined = cipher.doFinal(plaintext)
        val cipherLen = combined.size - TAG_LEN
        val ciphertext = ByteArray(cipherLen)
        val tag = ByteArray(TAG_LEN)
        System.arraycopy(combined, 0, ciphertext, 0, cipherLen)
        System.arraycopy(combined, cipherLen, tag, 0, TAG_LEN)
        return Pair(ciphertext, tag)
    }

    /**
     * ChaCha20-Poly1305 AEAD detached decryption.
     * Compatible with Conscrypt (Android) and SunJCE (JVM).
     */
    fun decryptWithAad(
        key: ByteArray,
        nonce: ByteArray,
        aad: ByteArray,
        ciphertext: ByteArray,
        tag: ByteArray
    ): ByteArray {
        val cipher = getChaChaCipher()
        val keySpec = createKeySpec(key)
        val paramSpec = IvParameterSpec(nonce)
        cipher.init(Cipher.DECRYPT_MODE, keySpec, paramSpec)
        if (aad.isNotEmpty()) {
            cipher.updateAAD(aad)
        }
        val combined = ByteArray(ciphertext.size + tag.size)
        System.arraycopy(ciphertext, 0, combined, 0, ciphertext.size)
        System.arraycopy(tag, 0, combined, ciphertext.size, tag.size)
        return cipher.doFinal(combined)
    }

    fun hexToBytes(hex: String): ByteArray {
        val clean = hex.trim()
        val len = clean.length
        val data = ByteArray(len / 2)
        var i = 0
        while (i < len) {
            data[i / 2] = ((Character.digit(clean[i], 16) shl 4) +
                    Character.digit(clean[i + 1], 16)).toByte()
            i += 2
        }
        return data
    }

    fun bytesToHex(bytes: ByteArray): String {
        val sb = StringBuilder()
        for (b in bytes) {
            sb.append(String.format("%02x", b))
        }
        return sb.toString()
    }
}
