package com.klip.app

import android.content.Context
import android.os.Build
import uniffi.klip_android.ClipboardBridge
import uniffi.klip_android.KlipEngine

/** Process-wide owner of the Rust engine (the service, tile, share targets and UI all share it). */
object KlipRuntime {
    private const val PREFS = "klip_prefs"
    private const val KEY_RELAYS = "use_relays"

    @Volatile
    var engine: KlipEngine? = null
        private set

    /** Publishes the app context to the Rust networking stack (system DNS lookup). Implemented in klip-android. */
    @JvmStatic
    private external fun installAndroidContext(context: Context)

    private val contextInstalled = java.util.concurrent.atomic.AtomicBoolean(false)

    private fun installContextOnce(ctx: Context) {
        if (contextInstalled.compareAndSet(false, true)) {
            System.loadLibrary("klip_android")
            installAndroidContext(ctx.applicationContext)
        }
    }

    private val lock = Any()

    /** What Klip itself last put on the clipboard (and when): used to tell its own writes apart from user copies. */
    @Volatile var lastWrittenText: String? = null
    @Volatile var lastWrittenAtMs: Long = 0

    fun recentlyWroteClipboard(withinMs: Long = 3000): Boolean =
        System.currentTimeMillis() - lastWrittenAtMs < withinMs

    fun deviceName(): String {
        val maker = Build.MANUFACTURER.replaceFirstChar { it.uppercase() }
        val model = Build.MODEL
        return if (model.startsWith(maker, ignoreCase = true)) model else "$maker $model"
    }

    /** Use n0's public relays so devices on different networks can find each other. */
    fun useRelays(ctx: Context): Boolean =
        ctx.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .getBoolean(KEY_RELAYS, true)

    fun setUseRelays(ctx: Context, enabled: Boolean) {
        ctx.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .edit().putBoolean(KEY_RELAYS, enabled).apply()
    }

    /** Starts the engine once. Blocking: call from a background thread. */
    fun ensureStarted(ctx: Context, bridge: ClipboardBridge): KlipEngine = synchronized(lock) {
        installContextOnce(ctx)
        engine ?: KlipEngine.start(
            ctx.applicationContext.filesDir.absolutePath,
            deviceName(),
            useRelays(ctx),
            bridge
        ).also { engine = it }
    }

    fun stop() = synchronized(lock) {
        engine?.let { runCatching { it.stop() } }
        engine = null
    }
}
