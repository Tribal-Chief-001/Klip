package com.klip.app

import android.app.Activity
import android.app.NotificationManager
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.PersistableBundle
import android.util.Log
import android.widget.Toast

/**
 * Transparent window that exists only to take focus for an instant (Android lets only the focused
 * app read the clipboard).
 *
 * - ACTION_SEND_CLIPBOARD: read the current clipboard and send it to the linked devices.
 * - ACTION_COPY: put a received text on the clipboard (tap-to-copy fallback).
 */
class CopyTrampolineActivity : Activity() {

    companion object {
        private const val TAG = "KlipTrampoline"
        const val ACTION_SEND_CLIPBOARD = "com.klip.app.ACTION_SEND_CLIPBOARD"
        const val ACTION_COPY = "com.klip.app.ACTION_COPY"
        const val EXTRA_CLIP_TEXT = "extra_clip_text"
        const val EXTRA_IS_SENSITIVE = "extra_is_sensitive"
        /** Do not show toasts (used when triggered automatically in the background). */
        const val EXTRA_QUIET = "extra_quiet"
    }

    private var handled = false
    private val main = Handler(Looper.getMainLooper())

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (intent.action == ACTION_COPY) {
            copyToClipboard()
            finishQuietly()
        }
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus && intent.action == ACTION_SEND_CLIPBOARD) runOnce()
    }

    override fun onResume() {
        super.onResume()
        // Some OEM builds deliver the focus callback late.
        if (intent.action == ACTION_SEND_CLIPBOARD) main.postDelayed({ runOnce() }, 150)
    }

    private fun runOnce() {
        if (handled) return
        handled = true
        readAndSend()
        finishQuietly()
    }

    private fun readAndSend() {
        val quiet = intent.getBooleanExtra(EXTRA_QUIET, false)
        val text = try {
            (getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager)
                .primaryClip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.text?.toString()
        } catch (e: Throwable) {
            null
        }
        val app = applicationContext
        if (text.isNullOrEmpty()) {
            if (!quiet) toast(app, "Klip: the clipboard is empty")
            return
        }
        val engine = KlipRuntime.engine
        if (engine == null) {
            KlipSyncService.start(app)
            if (!quiet) toast(app, "Klip is starting — try again in a moment")
            return
        }
        Thread {
            try {
                val report = engine.sendText(text)
                if (!quiet) {
                    val msg = when {
                        report.delivered.isNotEmpty() && report.failed.isEmpty() ->
                            "Klip: sent to ${report.delivered.joinToString()}"
                        report.delivered.isNotEmpty() ->
                            "Klip: sent to ${report.delivered.joinToString()} — failed: " +
                                report.failed.joinToString { "${it.device} (${it.reason})" }
                        report.failed.isNotEmpty() ->
                            "Klip: not delivered — " + report.failed.joinToString { "${it.device}: ${it.reason}" }
                        else -> "Klip: no linked devices yet"
                    }
                    toast(app, msg)
                }
            } catch (e: Throwable) {
                if (!quiet) toast(app, "Klip: ${e.message}")
            }
        }.start()
    }

    private fun copyToClipboard() {
        val text = intent.getStringExtra(EXTRA_CLIP_TEXT)
        val sensitive = intent.getBooleanExtra(EXTRA_IS_SENSITIVE, false)
        if (!text.isNullOrEmpty()) {
            try {
                val cm = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
                val clip = ClipData.newPlainText("Klip", text)
                if (sensitive) {
                    clip.description.extras = PersistableBundle().apply {
                        putBoolean("android.content.extra.IS_SENSITIVE", true)
                    }
                }
                cm.setPrimaryClip(clip)
                toast(applicationContext, "Copied")
            } catch (e: Throwable) {
                Log.e(TAG, "Copy failed: ${e.message}", e)
                toast(applicationContext, "Could not copy: ${e.message}")
            }
        }
        runCatching {
            (getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager)
                .cancel(KlipSyncService.NOTIFICATION_CLIP_ID)
        }
    }

    private fun toast(ctx: Context, msg: String) {
        main.post { Toast.makeText(ctx, msg, Toast.LENGTH_LONG).show() }
    }

    private fun finishQuietly() {
        finish()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            overrideActivityTransition(OVERRIDE_TRANSITION_CLOSE, 0, 0)
        } else {
            @Suppress("DEPRECATION")
            overridePendingTransition(0, 0)
        }
    }
}
