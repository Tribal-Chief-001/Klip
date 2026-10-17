package com.klip.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.ClipboardManager
import android.content.IntentFilter
import android.os.PowerManager
import android.widget.RemoteViews
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
import kotlinx.coroutines.withContext
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * Foreground service that keeps the Rust engine (and so the network connection) alive.
 *
 * Phone -> other devices: Android 10+ only lets the focused app read the clipboard, so the
 * automatic path works while Klip is in front; otherwise use the tile, the notification button,
 * "Send to Klip" in the text menu, or the optional clipboard helper (see AutoCopy).
 * Other devices -> phone: written straight to the clipboard by [ClipboardWriter].
 */
class KlipSyncService : Service() {

    companion object {
        const val ACTION_START = "com.klip.app.ACTION_START"
        const val ACTION_STOP = "com.klip.app.ACTION_STOP"
        /** The notification was swiped away: Android 14+ allows that for foreground services. Show it again. */
        const val ACTION_REPOST = "com.klip.app.ACTION_REPOST"
        const val ACTION_TEST_BG_WRITE = "com.klip.app.ACTION_TEST_BG_WRITE"

        const val CHANNEL_SERVICE = "klip_service"
        const val CHANNEL_CLIP = "klip_clip"
        const val NOTIFICATION_ID = 40401
        const val NOTIFICATION_CLIP_ID = 40402

        /** True while this process has the service running (a fresh process after a stop starts as false). */
        @Volatile var running = false
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
    private var started = false
    private var lastShown = ""
    private var note: String? = null
    private var noteUntil = 0L
    private lateinit var power: PowerManager

    /** While the screen is on, re-check the status every few seconds; while it is off, do nothing at all. */
    private val ticker = object : Runnable {
        override fun run() {
            refreshNotification()
            scope.launch { AutoCopy.startWithRoot(this@KlipSyncService) }
            if (power.isInteractive) main.postDelayed(this, 8_000)
        }
    }
    private val screenReceiver = object : BroadcastReceiver() {
        override fun onReceive(c: Context, i: Intent) {
            main.removeCallbacks(ticker)
            if (i.action == Intent.ACTION_SCREEN_ON) main.post(ticker)
        }
    }

    private val clipListener = ClipboardManager.OnPrimaryClipChangedListener { onClipChanged() }

    override fun onCreate() {
        super.onCreate()
        createChannels()
        showForeground("Starting…")
        clipboard = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
        power = getSystemService(Context.POWER_SERVICE) as PowerManager
        // Never put clip content in a notification: it can show on a locked screen.
        writer = ClipboardWriter(this) { _ -> refreshNotification() }
        clipboard.addPrimaryClipChangedListener(clipListener)
        acquireMulticastLock()
        startEngine()
        scope.launch { ClipImages.prune(this@KlipSyncService) }
        scope.launch { runCatching { FileReceiver.cleanStale(this@KlipSyncService) } }
        FileReceiver.ensureChannel(this)
        ClipHelperServer.start(this)
        scope.launch { delay(3_000); AutoCopy.startWithRoot(this@KlipSyncService) }
        registerReceiver(screenReceiver, IntentFilter().apply {
            addAction(Intent.ACTION_SCREEN_ON)
            addAction(Intent.ACTION_SCREEN_OFF)
        })
        if (power.isInteractive) main.post(ticker)
        Watchdog.schedule(this)
        running = true
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> {
                stopSelf()
                return START_NOT_STICKY
            }
            ACTION_TEST_BG_WRITE -> scheduleBackgroundWriteTest()
            ACTION_REPOST -> refreshNotification(force = true)
            else -> {
                if (!started) startEngine()
                refreshNotification() // e.g. notifications were switched back on while Klip was running
            }
        }
        return START_STICKY
    }

    private fun startEngine() {
        if (started) return
        started = true
        scope.launch {
            try {
                KlipRuntime.ensureStarted(this@KlipSyncService, writer)
                refreshNotification()
            } catch (e: Throwable) {
                started = false
                Log.e(TAG, "Engine failed to start: ${e.message}", e)
                setNote("Error: ${e.message}")
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
        setNote("Test: in 10 s a text is placed on the clipboard. Switch to another app and paste.")
        main.postDelayed({
            val stamp = java.text.SimpleDateFormat("HH:mm:ss", java.util.Locale.US).format(java.util.Date())
            val ok = writer.writeText("KLIP-BACKGROUND-TEST $stamp", false)
            setNote(if (ok) "Test text placed at $stamp. Paste it to check." else "Test write failed.")
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

    private fun buildNotification(summary: StatusText.Summary): Notification {
        val flags = PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), flags)
        val send = PendingIntent.getActivity(
            this, 1,
            Intent(this, CopyTrampolineActivity::class.java)
                .setAction(CopyTrampolineActivity.ACTION_SEND_CLIPBOARD)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            flags
        )
        val repost = PendingIntent.getService(
            this, 2, Intent(this, KlipSyncService::class.java).setAction(ACTION_REPOST), flags
        )
        val extra = note?.takeIf { System.currentTimeMillis() < noteUntil }
        val line = extra ?: summary.compact

        // One compact view only, with the status and the Send button: no expanded layout. Android alone
        // decides which notification is shown expanded by default (often the top one) and apps cannot
        // turn that off, so there is nothing larger to open and no arrow either.
        val view = RemoteViews(packageName, R.layout.notification_collapsed).apply {
            setTextViewText(R.id.title, summary.title.removePrefix("Klip · ")) // the icon and header already say Klip
            setTextViewText(R.id.line, line)
            setOnClickPendingIntent(R.id.send, send)
        }
        return NotificationCompat.Builder(this, CHANNEL_SERVICE)
            .setSmallIcon(R.drawable.ic_stat_klip)
            .setColor(0xFF4D7A00.toInt())
            .setContentTitle(summary.title)
            .setContentText(line)
            .setStyle(NotificationCompat.DecoratedCustomViewStyle())
            .setCustomContentView(view)
            .setContentIntent(open)
            .setDeleteIntent(repost)
            .setOngoing(true)
            .setShowWhen(false)
            .setOnlyAlertOnce(true)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .build()
    }

    private fun showForeground(text: String) =
        postForeground(StatusText.Summary("Klip", "Klip", text, text, text, false, null))

    /** (Re)attach the notification to the foreground service. Also puts it back when it was removed. */
    private fun postForeground(summary: StatusText.Summary) {
        val n = buildNotification(summary)
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

    /** A short extra line (used by the background-paste test). */
    private fun setNote(text: String) {
        note = text
        noteUntil = System.currentTimeMillis() + 20_000
        refreshNotification(force = true)
    }

    /**
     * Recompute the status off the main thread and update the notification when it changed. It is also put
     * back when it has vanished without us touching it: the person switched notifications off and on in
     * Settings, blocked and unblocked the channel, or swiped it away. (Android does not bring it back by itself.)
     */
    private fun refreshNotification(force: Boolean = false) {
        scope.launch {
            val summary = withContext(Dispatchers.IO) { runCatching { StatusText.compute() }.getOrNull() } ?: return@launch
            val key = summary.key + (note?.takeIf { System.currentTimeMillis() < noteUntil } ?: "")
            val nm = getSystemService(NotificationManager::class.java)
            val missing = nm.areNotificationsEnabled() &&
                runCatching { nm.activeNotifications.none { it.id == NOTIFICATION_ID } }.getOrDefault(false)
            if (!force && !missing && key == lastShown) return@launch
            lastShown = key
            withContext(Dispatchers.Main) {
                if (missing) postForeground(summary) else nm.notify(NOTIFICATION_ID, buildNotification(summary))
            }
        }
    }

    override fun onDestroy() {
        super.onDestroy()
        running = false
        main.removeCallbacks(ticker)
        runCatching { unregisterReceiver(screenReceiver) }
        runCatching { clipboard.removePrimaryClipChangedListener(clipListener) }
        runCatching { multicastLock?.takeIf { it.isHeld }?.release() }
        scope.cancel()
        // Stopping the engine waits for the network to shut down; never block the main thread.
        Thread({ KlipRuntime.stop() }, "klip-stop").start()
    }

    override fun onBind(intent: Intent?): IBinder? = null
}
