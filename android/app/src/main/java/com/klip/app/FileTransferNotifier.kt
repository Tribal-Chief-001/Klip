package com.klip.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import androidx.core.app.NotificationCompat
import uniffi.klip_android.TransferStateView

/**
 * One progress notification for all running file transfers, with a Cancel action. It looks at the engine once a
 * second, but only while something is running (the engine says when a transfer begins or ends) and holds a wake
 * lock and a Wi-Fi lock only for that time. The notification never shows a file name.
 */
class FileTransferNotifier(private val ctx: Context) {
    companion object {
        const val CHANNEL = "klip_transfers"
        const val NOTIFICATION_ID = 40403
        const val ACTION_CANCEL_ALL = "com.klip.app.ACTION_CANCEL_TRANSFERS"
        private const val LOCK_MS = 10 * 60_000L
    }

    private val main = Handler(Looper.getMainLooper())
    private var polling = false
    private var wake: PowerManager.WakeLock? = null
    private var wifi: WifiManager.WifiLock? = null

    private val tick = object : Runnable {
        override fun run() {
            val running = update()
            if (running) main.postDelayed(this, 1000) else { polling = false; releaseLocks() }
        }
    }

    /** The engine says something began or ended: start looking (once). */
    fun changed() {
        main.post {
            if (!polling) { polling = true; tick.run() }
        }
    }

    fun stop() {
        main.removeCallbacksAndMessages(null)
        polling = false
        releaseLocks()
        ctx.getSystemService(NotificationManager::class.java).cancel(NOTIFICATION_ID)
    }

    /** Returns true while a transfer is running. */
    private fun update(): Boolean {
        val engine = KlipRuntime.engine ?: return false
        val rows = runCatching { engine.transfers() }.getOrNull() ?: return false
        val running = rows.filter { it.state == TransferStateView.WAITING || it.state == TransferStateView.ACTIVE }
        val nm = ctx.getSystemService(NotificationManager::class.java)
        if (running.isEmpty()) {
            nm.cancel(NOTIFICATION_ID)
            return false
        }
        acquireLocks()
        val line = FileText.progress(rows.map {
            FileText.Row(it.outgoing, it.size.toLong(), it.done.toLong(), it.state == TransferStateView.WAITING || it.state == TransferStateView.ACTIVE)
        })
        val total = running.sumOf { it.size.toLong() }
        val done = running.sumOf { it.done.toLong() }
        ensureChannel()
        val cancel = PendingIntent.getService(
            ctx, 3, Intent(ctx, KlipSyncService::class.java).setAction(ACTION_CANCEL_ALL),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        )
        val n = NotificationCompat.Builder(ctx, CHANNEL)
            .setSmallIcon(R.drawable.ic_stat_klip)
            .setContentTitle("Klip")
            .setContentText(line)
            .setProgress(1000, if (total == 0L) 0 else (done * 1000 / total).toInt().coerceIn(0, 1000), total == 0L)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setSilent(true)
            .setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
            .addAction(0, "Cancel", cancel)
            .build()
        nm.notify(NOTIFICATION_ID, n)
        return true
    }

    fun cancelAll() {
        val engine = KlipRuntime.engine ?: return
        runCatching {
            engine.transfers()
                .filter { it.state == TransferStateView.WAITING || it.state == TransferStateView.ACTIVE }
                .forEach { engine.cancelTransfer(it.id) }
        }
    }

    private fun ensureChannel() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        ctx.getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(CHANNEL, "File transfers", NotificationManager.IMPORTANCE_LOW).apply {
                description = "Progress of files being sent or received"
                lockscreenVisibility = Notification.VISIBILITY_PRIVATE
                setShowBadge(false)
            }
        )
    }

    private fun acquireLocks() {
        runCatching {
            val pm = ctx.getSystemService(Context.POWER_SERVICE) as PowerManager
            val w = wake ?: pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "klip:transfer").also { wake = it; it.setReferenceCounted(false) }
            w.acquire(LOCK_MS)
            val wm = ctx.applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
            val l = wifi ?: wm.createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "klip:transfer").also { wifi = it; it.setReferenceCounted(false) }
            if (!l.isHeld) l.acquire()
        }
    }

    private fun releaseLocks() {
        runCatching { wake?.takeIf { it.isHeld }?.release() }
        runCatching { wifi?.takeIf { it.isHeld }?.release() }
    }
}
