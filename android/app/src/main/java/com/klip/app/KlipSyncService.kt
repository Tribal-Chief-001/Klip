package com.klip.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.provider.Settings
import android.util.Log
import androidx.core.app.NotificationCompat
import kotlinx.coroutines.CoroutineExceptionHandler
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch

/**
 * Foreground service that keeps the Rust engine (and so the network connection) alive.
 *
 * Phone -> other devices: Android 10+ only lets the focused app read the clipboard, so the
 * automatic path works while Klip is in front; otherwise use the tile, the notification button,
 * "Send to Klip" in the text menu, or the optional background mode (see ClipboardLogWatcher).
 * Other devices -> phone: written straight to the clipboard by [ClipboardWriter].
 */
class KlipSyncService : Service() {

    companion object {
        const val ACTION_START = "com.klip.app.ACTION_START"
        const val ACTION_STOP = "com.klip.app.ACTION_STOP"
        const val ACTION_TEST_BG_WRITE = "com.klip.app.ACTION_TEST_BG_WRITE"

        const val CHANNEL_SERVICE = "klip_service"
        const val CHANNEL_CLIP = "klip_clip"
        const val NOTIFICATION_ID = 40401
        const val NOTIFICATION_CLIP_ID = 40402
        private const val TAG = "KlipSyncService"

        /** Stop and start again, e.g. after the relay setting changed. */
        fun restart(ctx: Context) {
            ctx.stopService(Intent(ctx, KlipSyncService::class.java))
            Handler(Looper.getMainLooper()).postDelayed({ start(ctx) }, 600)
        }

        fun start(ctx: Context) {
            val intent = Intent(ctx, KlipSyncService::class.java).setAction(ACTION_START)
            try {
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                    ctx.startForegroundService(intent)
                } else {
                    ctx.startService(intent)
                }
            } catch (e: Throwable) {
                Log.e(TAG, "Cannot start service: ${e.message}", e)
            }
        }
    }

    private val scope = CoroutineScope(
        SupervisorJob() + Dispatchers.IO +
            CoroutineExceptionHandler { _, t -> Log.e(TAG, "Background error: ${t.message}", t) }
    )
    private val main = Handler(Looper.getMainLooper())
    private lateinit var clipboard: ClipboardManager
    private lateinit var writer: ClipboardWriter
    private var multicastLock: WifiManager.MulticastLock? = null
    private var logWatcher: ClipboardLogWatcher? = null
    private var started = false

    private val clipListener = ClipboardManager.OnPrimaryClipChangedListener { onClipChanged() }

    override fun onCreate() {
        super.onCreate()
        createChannels()
        showForeground("Starting…")
        clipboard = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
        writer = ClipboardWriter(this) { preview -> notify("Last received: $preview") }
        clipboard.addPrimaryClipChangedListener(clipListener)
        acquireMulticastLock()
        startEngine()
        startLogWatcher()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> {
                stopSelf()
                return START_NOT_STICKY
            }
            ACTION_TEST_BG_WRITE -> scheduleBackgroundWriteTest()
            else -> if (!started) startEngine()
        }
        return START_STICKY
    }

    private fun startEngine() {
        if (started) return
        started = true
        scope.launch {
            try {
                val engine = KlipRuntime.ensureStarted(this@KlipSyncService, writer)
                val peers = engine.status().peers.size
                notify(if (peers == 0) "Running — not linked to any device yet" else "Running — linked to $peers device(s)")
            } catch (e: Throwable) {
                started = false
                Log.e(TAG, "Engine failed to start: ${e.message}", e)
                notify("Error: ${e.message}")
            }
        }
    }

    private fun startLogWatcher() {
        val watcher = ClipboardLogWatcher(this) { launchClipboardReader() }
        if (watcher.isPermitted()) {
            watcher.start()
            logWatcher = watcher
        } else {
            Log.i(TAG, "automatic copy mode off (READ_LOGS not granted)")
        }
    }

    /** A background copy was refused by Android: bring up the transparent reader window. */
    private fun launchClipboardReader() {
        if (!Settings.canDrawOverlays(this)) {
            Log.w(TAG, "automatic mode needs the 'display over other apps' permission")
            return
        }
        // Our own write (a clip that just arrived) also triggers a refused read: nothing to send.
        if (KlipRuntime.recentlyWroteClipboard()) {
            Log.i(TAG, "ignoring refused read caused by Klip's own clipboard write")
            return
        }
        main.post {
            try {
                startActivity(
                    Intent(this, CopyTrampolineActivity::class.java)
                        .setAction(CopyTrampolineActivity.ACTION_SEND_CLIPBOARD)
                        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_NO_ANIMATION)
                        .putExtra(CopyTrampolineActivity.EXTRA_QUIET, true)
                )
            } catch (e: Throwable) {
                Log.w(TAG, "Cannot open clipboard reader: ${e.message}")
            }
        }
    }

    /** Called on the main thread when the clipboard changed. Works while Klip has focus. */
    private fun onClipChanged() {
        val text = try {
            clipboard.primaryClip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.text?.toString()
        } catch (e: Throwable) {
            null // refused (not focused): the optional log watcher reacts to this
        }
        if (!text.isNullOrEmpty()) {
            scope.launch { KlipRuntime.engine?.onLocalClip(text) }
        }
    }

    /** Lets the user check whether this phone allows writing to the clipboard from the background. */
    private fun scheduleBackgroundWriteTest() {
        notify("Test: a text will be placed on the clipboard in 10 s — switch to another app and paste")
        main.postDelayed({
            val stamp = java.text.SimpleDateFormat("HH:mm:ss", java.util.Locale.US).format(java.util.Date())
            val ok = writer.writeText("KLIP-BACKGROUND-TEST $stamp", false)
            notify(if (ok) "Test text placed at $stamp — paste it to check" else "Test write failed")
        }, 10_000)
    }

    private fun acquireMulticastLock() {
        try {
            val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
            multicastLock = wifi.createMulticastLock("klip-mdns").apply {
                setReferenceCounted(false)
                acquire()
            }
        } catch (e: Throwable) {
            Log.w(TAG, "No multicast lock (LAN discovery may not work): ${e.message}")
        }
    }

    // ------------------------------------------------------------------------ notifications

    private fun createChannels() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val nm = getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL_SERVICE, "Klip running", NotificationManager.IMPORTANCE_LOW).apply {
                description = "Shows that Klip is running in the background"
                setShowBadge(false)
            }
        )
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL_CLIP, "Tap to copy", NotificationManager.IMPORTANCE_DEFAULT).apply {
                description = "Only used if Android refuses a direct clipboard write"
            }
        )
    }

    private fun buildNotification(text: String): Notification {
        val open = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        )
        val send = PendingIntent.getActivity(
            this, 1,
            Intent(this, CopyTrampolineActivity::class.java)
                .setAction(CopyTrampolineActivity.ACTION_SEND_CLIPBOARD)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        )
        return NotificationCompat.Builder(this, CHANNEL_SERVICE)
            .setSmallIcon(R.drawable.ic_stat_klip)
            .setContentTitle("Klip")
            .setContentText(text)
            .setStyle(NotificationCompat.BigTextStyle().bigText(text))
            .setContentIntent(open)
            .addAction(0, "Send clipboard", send)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .build()
    }

    private fun showForeground(text: String) {
        val n = buildNotification(text)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            // dataSync has a 6 h/24 h limit and cannot start from BOOT_COMPLETED once an app targets
            // Android 15; an always-on peer connection is what specialUse is for.
            startForeground(NOTIFICATION_ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        } else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(NOTIFICATION_ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            startForeground(NOTIFICATION_ID, n)
        }
    }

    private fun notify(text: String) {
        (getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager)
            .notify(NOTIFICATION_ID, buildNotification(text))
    }

    override fun onDestroy() {
        super.onDestroy()
        runCatching { clipboard.removePrimaryClipChangedListener(clipListener) }
        logWatcher?.stop()
        runCatching { multicastLock?.takeIf { it.isHeld }?.release() }
        scope.cancel()
        // Stopping the engine waits for the network to shut down; never block the main thread.
        Thread({ KlipRuntime.stop() }, "klip-stop").start()
    }

    override fun onBind(intent: Intent?): IBinder? = null
}
