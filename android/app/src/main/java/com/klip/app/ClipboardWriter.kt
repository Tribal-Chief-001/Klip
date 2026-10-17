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
        // This is called from the Rust engine. An exception escaping from here would abort the whole app
        // (the Android engine is built with panic = "abort"), so nothing may be thrown out of this function.
        try {
            // Android passes the clip to the clipboard service in one Binder transaction, whose buffer is
            // about 1 MB for the whole process: a bigger clip fails with TransactionTooLargeException (seen at
            // 1.18 MB, accepted at 0.5 MB on Android 14). Say no early instead of failing half way.
            val size = text.toByteArray(Charsets.UTF_8).size
            if (size > MAX_CLIPBOARD_BYTES) {
                Log.w(TAG, "clip of $size bytes is too large for Android's clipboard (limit about 1 MB); not applied")
                return false
            }
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
            try {
                cm.setPrimaryClip(clip)
            } catch (e: Throwable) {
                Log.w(TAG, "Direct clipboard write failed, offering tap-to-copy: ${e.message}")
                if (size > MAX_TAP_TO_COPY_BYTES || !offerTapToCopy(text, sensitive)) return false
            }
            onApplied(if (sensitive) "secret (hidden)" else text.take(60))
            return true
        } catch (e: Throwable) {
            Log.w(TAG, "clipboard write failed: ${e.message}")
            return false
        }
    }

    override fun writeImage(data: ByteArray, mime: String): Boolean {
        // Like writeText: nothing may be thrown out of here (the engine is built with panic = abort).
        return try {
            val uri = ClipImages.store(ctx, data, mime)
            val cm = ctx.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
            cm.setPrimaryClip(ClipData.newUri(ctx.contentResolver, "Klip image", uri))
            onApplied("image (${data.size / 1024} KB)")
            true
        } catch (e: Throwable) {
            Log.w(TAG, "image clipboard write failed: ${e.message}")
            false
        }
    }

    override fun fileReceived(path: String, name: String, size: ULong, from: String, relDir: String) {
        FileReceiver.onReceived(ctx, path, name, size.toLong(), from, relDir)
    }

    override fun transfersChanged() {
        try {
            KlipRuntime.transfersChanged?.invoke()
        } catch (e: Throwable) {
            Log.w(TAG, "transfer notification failed: ${e.message}")
        }
    }

    /** The text travels in the notification's intent (a Binder transaction too), so only small clips use it. */
    private fun offerTapToCopy(text: String, sensitive: Boolean): Boolean = try {
        postTapToCopy(text, sensitive)
        true
    } catch (e: Throwable) {
        Log.w(TAG, "tap-to-copy notification failed: ${e.message}")
        false
    }

    private fun postTapToCopy(text: String, sensitive: Boolean) {
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
            .setSmallIcon(R.drawable.ic_stat_klip)
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
        private const val MAX_CLIPBOARD_BYTES = 900_000
        private const val MAX_TAP_TO_COPY_BYTES = 100_000
    }
}
