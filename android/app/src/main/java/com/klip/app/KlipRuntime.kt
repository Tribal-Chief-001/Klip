package com.klip.app

import android.content.Context
import android.os.Build
import uniffi.klip_android.ClipboardBridge
import uniffi.klip_android.KlipEngine

/** Process-wide owner of the Rust engine (the service, tile, share targets and UI all share it). */
object KlipRuntime {
    private const val PREFS = "klip_prefs"
    private const val KEY_RELAYS = "use_relays"
    private const val KEY_RECEIVE_FILES = "receive_files"
    private const val KEY_MAX_FILE_MB = "max_file_mb"
    /** Phones have less room than computers, and a received file exists twice for a moment. */
    const val DEFAULT_MAX_FILE_MB = 2048

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

    /** Set by the service: the engine says a file transfer began or ended. */
    @Volatile var transfersChanged: (() -> Unit)? = null

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

    fun receiveFiles(ctx: Context): Boolean =
        ctx.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .getBoolean(KEY_RECEIVE_FILES, true)

    fun setReceiveFiles(ctx: Context, enabled: Boolean) {
        ctx.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .edit().putBoolean(KEY_RECEIVE_FILES, enabled).apply()
    }

    fun maxFileMb(ctx: Context): Int =
        ctx.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .getInt(KEY_MAX_FILE_MB, DEFAULT_MAX_FILE_MB)

    fun setMaxFileMb(ctx: Context, mb: Int) {
        ctx.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .edit().putInt(KEY_MAX_FILE_MB, mb).apply()
    }

    // ------------------------------------------------------------------------------------- pause
    // The engine keeps its pause in memory only, so the deadline is also stored here: a pause then
    // survives the process being restarted (by Android, by the watchdog, by an update). Long.MAX_VALUE
    // means "until the person resumes".

    private const val KEY_PAUSE_UNTIL = "pause_until_ms"
    const val PAUSE_FOREVER = Long.MAX_VALUE
    private const val TEN_YEARS_SECS = 10L * 365 * 24 * 3600

    /** When the pause ends (epoch ms), [PAUSE_FOREVER], or 0 when sync is not paused. Clears an expired pause. */
    fun pauseUntilMs(ctx: Context): Long {
        val p = ctx.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val until = p.getLong(KEY_PAUSE_UNTIL, 0)
        if (until != 0L && until != PAUSE_FOREVER && until <= System.currentTimeMillis()) {
            p.edit().remove(KEY_PAUSE_UNTIL).apply()
            return 0
        }
        return until
    }

    /** Pause for [seconds], or until resumed when null. Blocking (engine call): use a background thread. */
    fun pause(ctx: Context, seconds: Long?) {
        val until = if (seconds == null) PAUSE_FOREVER else System.currentTimeMillis() + seconds * 1000
        ctx.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .edit().putLong(KEY_PAUSE_UNTIL, until).apply()
        engine?.pause((seconds ?: TEN_YEARS_SECS).toULong())
    }

    fun resume(ctx: Context) {
        ctx.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .edit().remove(KEY_PAUSE_UNTIL).apply()
        engine?.resume()
    }

    private fun applyStoredPause(ctx: Context, e: KlipEngine) {
        val until = pauseUntilMs(ctx)
        if (until == 0L) return
        val secs = if (until == PAUSE_FOREVER) TEN_YEARS_SECS else (until - System.currentTimeMillis()) / 1000
        if (secs > 0) e.pause(secs.toULong())
    }

    /** Starts the engine once. Blocking: call from a background thread. */
    fun ensureStarted(ctx: Context, bridge: ClipboardBridge): KlipEngine = synchronized(lock) {
        installContextOnce(ctx)
        engine ?: KlipEngine.start(
            ctx.applicationContext.filesDir.absolutePath,
            deviceName(),
            useRelays(ctx),
            bridge,
            receiveFiles(ctx),
            maxFileMb(ctx).toULong() * 1024uL * 1024uL
        ).also { engine = it; applyStoredPause(ctx, it) }
    }

    /** True while a file is being sent or received. */
    fun transferRunning(): Boolean =
        runCatching {
            engine?.transfers()?.any {
                it.state == uniffi.klip_android.TransferStateView.WAITING || it.state == uniffi.klip_android.TransferStateView.ACTIVE
            } ?: false
        }.getOrDefault(false)

    fun stop() = synchronized(lock) {
        engine?.let { runCatching { it.stop() } }
        engine = null
    }
}
