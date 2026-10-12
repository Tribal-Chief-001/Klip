package com.klip.app

import android.app.NotificationManager
import android.app.PendingIntent
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.os.PersistableBundle
import android.util.Log
import androidx.core.app.NotificationCompat
import uniffi.klip_android.ClipboardBridge

/**
 * Lets the Rust engine put a clip from another device onto the Android clipboard.
 *
 * Writing is attempted directly. If the system refuses (some OEM builds), a "tap to copy"
 * notification is posted instead, so the clip is never silently lost.
 */
class ClipboardWriter(
    private val ctx: Context,
    private val onApplied: (preview: String) -> Unit
) : ClipboardBridge {

    @Volatile
    var lastWritten: String? = null
        private set

    override fun writeText(text: String, sensitive: Boolean): Boolean {
        return try {
            val cm = ctx.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
            val clip = ClipData.newPlainText("Klip", text)
            if (sensitive) {
                // Keyboards then hide the preview of this clip (honoured on Android 13+).
                clip.description.extras = PersistableBundle().apply {
                    putBoolean("android.content.extra.IS_SENSITIVE", true)
                }
            }
            lastWritten = text
            KlipRuntime.lastWrittenText = text
            KlipRuntime.lastWrittenAtMs = System.currentTimeMillis()
            cm.setPrimaryClip(clip)
            onApplied(if (sensitive) "secret (hidden)" else text.take(60))
            true
        } catch (e: Throwable) {
            Log.w(TAG, "Direct clipboard write failed, offering tap-to-copy: ${e.message}")
            offerTapToCopy(text, sensitive)
            true // delivered to this device; the user finishes it with one tap
        }
    }

    private fun offerTapToCopy(text: String, sensitive: Boolean) {
        val intent = Intent(ctx, CopyTrampolineActivity::class.java).apply {
            action = CopyTrampolineActivity.ACTION_COPY
            flags = Intent.FLAG_ACTIVITY_NEW_TASK
            putExtra(CopyTrampolineActivity.EXTRA_CLIP_TEXT, text)
            putExtra(CopyTrampolineActivity.EXTRA_IS_SENSITIVE, sensitive)
        }
        val pi = PendingIntent.getActivity(
            ctx, System.currentTimeMillis().toInt(), intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        )
        val shown = if (sensitive) "Secret (tap to copy)" else text.take(80)
        val n = NotificationCompat.Builder(ctx, KlipSyncService.CHANNEL_CLIP)
            .setSmallIcon(android.R.drawable.ic_menu_agenda)
            .setContentTitle("Klip: tap to copy")
            .setContentText(shown)
            .setAutoCancel(true)
            .setContentIntent(pi)
            .setTimeoutAfter(5 * 60 * 1000L)
            .build()
        (ctx.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager)
            .notify(KlipSyncService.NOTIFICATION_CLIP_ID, n)
    }

    companion object {
        private const val TAG = "KlipClipboardWriter"
    }
}
