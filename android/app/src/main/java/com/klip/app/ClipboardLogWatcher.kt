package com.klip.app

import android.content.Context
import android.content.pm.PackageManager
import android.util.Log

/**
 * Optional "background clipboard" mode (the technique KDE Connect uses).
 *
 * Since Android 10 only the focused app may read the clipboard. When Klip's listener fires in the
 * background and the read is refused, Android logs a line naming our package. If the user granted
 * READ_LOGS over adb, we notice that line and briefly show a transparent window (which gives us
 * focus) so the clip can be read. Only lines containing our own package name are acted upon.
 */
class ClipboardLogWatcher(
    private val ctx: Context,
    private val onDenied: () -> Unit
) {
    @Volatile private var process: Process? = null
    @Volatile private var running = false
    private var lastTrigger = 0L

    fun isPermitted(): Boolean =
        ctx.checkSelfPermission(android.Manifest.permission.READ_LOGS) == PackageManager.PERMISSION_GRANTED

    fun start() {
        if (running || !isPermitted()) return
        running = true
        Thread({
            try {
                val p = Runtime.getRuntime().exec(
                    arrayOf("logcat", "-T", "1", "-v", "brief", "ClipboardService:V", "*:S")
                )
                process = p
                p.inputStream.bufferedReader().forEachLine { line ->
                    if (!running) return@forEachLine
                    if (line.contains(ctx.packageName) && line.contains("Denying clipboard access")) {
                        val now = System.currentTimeMillis()
                        if (now - lastTrigger > 800) {
                            lastTrigger = now
                            onDenied()
                        }
                    }
                }
            } catch (e: Throwable) {
                Log.w(TAG, "log watcher stopped: ${e.message}")
            } finally {
                running = false
            }
        }, "klip-log-watcher").apply { isDaemon = true }.start()
    }

    fun stop() {
        running = false
        runCatching { process?.destroy() }
        process = null
    }

    companion object {
        private const val TAG = "KlipLogWatcher"
    }
}
