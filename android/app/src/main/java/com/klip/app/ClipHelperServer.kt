package com.klip.app

import android.content.Context
import android.util.Log
import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.IOException
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.security.MessageDigest
import java.security.SecureRandom
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

/**
 * Receives copied text from the optional clipboard helper (see helper/ClipHelper.java), which runs as the
 * Android shell user or root and can therefore watch the clipboard while Klip is in the background.
 *
 * Loopback TCP: SELinux does not let the shell user connect to an app's Unix socket. Any app may open a
 * connection to a loopback port, so a connection is accepted only after the helper proves it knows the
 * secret token (HMAC challenge-response, token never sent), and the helper checks the same about us before
 * it sends anything. Clips from a connection that failed the proof are never read.
 */
object ClipHelperServer {
    private const val TAG = "KlipHelperServer"
    const val PORT = 47931
    private const val MAX_BYTES = 8 * 1024 * 1024
    private const val PREFS = "klip_ui"
    private const val KEY_TOKEN = "helper_token"

    @Volatile private var started = false
    @Volatile private var current: Socket? = null
    @Volatile private var token: ByteArray = ByteArray(0)

    /** True while a verified helper is connected. */
    @Volatile var connected = false
        private set

    /** The secret that goes into the helper's command line. Created once per install. */
    fun tokenText(ctx: Context): String {
        val p = ctx.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        p.getString(KEY_TOKEN, null)?.let { return it }
        val bytes = ByteArray(24).also { SecureRandom().nextBytes(it) }
        val text = bytes.joinToString("") { "%02x".format(it) }
        p.edit().putString(KEY_TOKEN, text).apply()
        return text
    }

    @Synchronized
    fun start(ctx: Context) {
        if (started) return
        token = tokenText(ctx).toByteArray(Charsets.UTF_8)
        started = true
        Thread({ serve() }, "klip-helper-server").apply { isDaemon = true }.start()
    }

    private fun serve() {
        val server = try {
            ServerSocket(PORT, 2, InetAddress.getByName("127.0.0.1"))
        } catch (e: IOException) {
            Log.w(TAG, "cannot listen on port $PORT for the helper: ${e.message}")
            started = false
            return
        }
        while (true) {
            val client = try { server.accept() } catch (e: IOException) { break }
            Thread({ handle(client) }, "klip-helper-link").apply { isDaemon = true }.start()
        }
        started = false
    }

    private fun handle(c: Socket) {
        try {
            c.tcpNoDelay = true
            c.soTimeout = 5_000 // the proof must arrive promptly
            val input = DataInputStream(c.getInputStream())
            val output = DataOutputStream(c.getOutputStream())
            if (!verifyHelper(input, output)) {
                Log.w(TAG, "a connection on the helper port failed the proof and was closed")
                return
            }
            c.soTimeout = 0
            current?.let { runCatching { it.close() } } // a new helper replaces an old one
            current = c
            connected = true
            Log.i(TAG, "clipboard helper connected")
            while (true) {
                val len = input.readInt()
                if (len <= 0 || len > MAX_BYTES) break
                val bytes = ByteArray(len)
                input.readFully(bytes)
                deliver(String(bytes, Charsets.UTF_8))
            }
        } catch (e: IOException) {
            // helper went away
        } finally {
            if (current === c) {
                current = null
                connected = false
            }
            runCatching { c.close() }
        }
    }

    private fun verifyHelper(input: DataInputStream, output: DataOutputStream): Boolean {
        val nh = ByteArray(16).also { input.readFully(it) }
        val ns = ByteArray(16).also { SecureRandom().nextBytes(it) }
        output.write(ns)
        output.write(mac("server", nh, ns))
        output.flush()
        val proof = ByteArray(32).also { input.readFully(it) }
        return MessageDigest.isEqual(proof, mac("helper", ns, nh))
    }

    private fun mac(role: String, a: ByteArray, b: ByteArray): ByteArray {
        val m = Mac.getInstance("HmacSHA256")
        m.init(SecretKeySpec(token, "HmacSHA256"))
        m.update(role.toByteArray(Charsets.UTF_8))
        m.update(a)
        m.update(b)
        return m.doFinal()
    }

    private fun deliver(text: String) {
        // Klip writes received clips to the clipboard itself; do not send those back.
        if (text == KlipRuntime.lastWrittenText && KlipRuntime.recentlyWroteClipboard(15_000)) {
            Log.i(TAG, "ignored a copy that Klip itself wrote (${text.length} chars)")
            return
        }
        val engine = KlipRuntime.engine
        Log.i(TAG, "helper delivered a copy of ${text.length} chars; engine ${if (engine == null) "NOT ready" else "ready"}")
        engine?.onLocalClip(text)
    }

    /** Ask the connected helper to exit. Safe to call from any thread (the write happens on a worker). */
    fun stopHelper() {
        val c = current ?: return
        Thread({
            try {
                c.getOutputStream().apply { write('Q'.code); flush() }
            } catch (e: IOException) {
                Log.w(TAG, "could not ask the helper to stop: ${e.message}")
            }
        }, "klip-helper-stop").start()
    }
}
