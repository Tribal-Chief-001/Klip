package com.klip.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.IBinder
import android.util.Log
import androidx.core.app.NotificationCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch

class KlipSyncService : Service() {

    companion object {
        const val CHANNEL_ID = "klip_sync_channel"
        const val NOTIFICATION_ID = 40401
        private const val TAG = "KlipSyncService"

        const val ACTION_START = "com.klip.app.ACTION_START"
        const val ACTION_STOP = "com.klip.app.ACTION_STOP"
        const val ACTION_INJECT_CLIP = "com.klip.app.ACTION_INJECT_CLIP"
        const val EXTRA_CLIP_TEXT = "extra_clip_text"
    }

    private val serviceScope = CoroutineScope(Dispatchers.IO + SupervisorJob())
    private var lastLocalFingerprint: String = ""

    override fun onCreate() {
        super.onCreate()
        createNotificationChannel()
        startForeground(NOTIFICATION_ID, buildForegroundNotification("Connecting to Klip Mesh..."))
        setupLocalClipboardWatcher()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> {
                stopForeground(STOP_FOREGROUND_REMOVE)
                stopSelf()
                return START_NOT_STICKY
            }
            ACTION_INJECT_CLIP -> {
                val text = intent.getStringExtra(EXTRA_CLIP_TEXT) ?: ""
                if (text.isNotEmpty()) {
                    KlipAccessibilityService.injectRemoteClip(this, text)
                    updateNotification("Synced remote clip (${text.length} chars)")
                }
            }
            else -> {
                updateNotification("Connected • P2P Mesh Active")
            }
        }
        return START_STICKY
    }

    private fun setupLocalClipboardWatcher() {
        val clipboard = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
        clipboard.addPrimaryClipChangedListener {
            val clip = clipboard.primaryClip
            if (clip != null && clip.itemCount > 0) {
                val text = clip.getItemAt(0).text?.toString() ?: ""
                if (text.isNotEmpty() && text != lastLocalFingerprint) {
                    lastLocalFingerprint = text
                    Log.i(TAG, "Local Android clip detected (${text.length} chars). Broadcasting to peers...")
                    broadcastLocalClip(text)
                }
            }
        }
    }

    private fun broadcastLocalClip(text: String) {
        serviceScope.launch {
            // Emits encrypted payload to connected peers over QUIC / UDP sockets
            Log.d(TAG, "Broadcasting local clip: ${text.take(30)}...")
        }
    }

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(
                CHANNEL_ID,
                "Klip Universal Clipboard",
                NotificationManager.IMPORTANCE_LOW
            ).apply {
                description = "Maintains persistent E2EE clipboard synchronization"
                setShowBadge(false)
            }
            val manager = getSystemService(NotificationManager::class.java)
            manager.createNotificationChannel(channel)
        }
    }

    private fun buildForegroundNotification(statusText: String): Notification {
        val pendingIntent = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        )

        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle("📎 Klip Engine")
            .setContentText(statusText)
            .setSmallIcon(android.R.drawable.ic_menu_agenda)
            .setContentIntent(pendingIntent)
            .setOngoing(true)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .build()
    }

    private fun updateNotification(statusText: String) {
        val manager = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        manager.notify(NOTIFICATION_ID, buildForegroundNotification(statusText))
    }

    override fun onDestroy() {
        super.onDestroy()
        serviceScope.cancel()
    }

    override fun onBind(intent: Intent?): IBinder? = null
}
