package com.klip.app

import android.app.DownloadManager
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.Environment
import android.provider.MediaStore
import android.util.Log
import androidx.core.app.NotificationCompat
import java.io.File
import java.util.concurrent.atomic.AtomicInteger

/**
 * A file from a linked device arrives in the app-private staging folder (`filesDir/incoming`). This moves
 * it to `Download/Klip` (visible in the Files and Downloads apps), checks the copy, and only then deletes
 * the staged file. If anything fails the staged file stays and the notification says so.
 * Nothing may be thrown out of here: the engine is built with panic = abort.
 */
object FileReceiver {
    private const val TAG = "KlipFileReceiver"
    const val CHANNEL = "klip_files"
    private val nextNotification = AtomicInteger(41000)

    /** Files from one device that arrive close together (a folder) share one notification that counts up. */
    private class Run(val id: Int, var count: Int, var bytes: Long, var lastMs: Long)
    private val runs = HashMap<String, Run>()
    private const val RUN_GAP_MS = 15_000L

    @Synchronized
    private fun noticeFor(from: String, size: Long): Pair<Int, String> {
        val now = System.currentTimeMillis()
        val run = runs[from]?.takeIf { now - it.lastMs < RUN_GAP_MS }
            ?: Run(nextNotification.getAndIncrement(), 0, 0, now).also { runs[from] = it }
        run.count++
        run.bytes += size
        run.lastMs = now
        return run.id to FileText.receivedBatch(from, run.count, run.bytes)
    }

    fun onReceived(ctx: Context, stagedPath: String, name: String, size: Long, from: String, relDir: String) {
        val app = ctx.applicationContext
        try {
            val staged = File(stagedPath)
            val rel = FileText.safeRelDir(relDir)
            val shown = FileText.safeName(name)
            if (copyToDownloads(app, staged, shown, rel)) {
                staged.delete()
                pruneEmptyParents(app, staged.parentFile)
                val (id, text) = noticeFor(from, size)
                notify(app, text, downloads(app), id)
            } else {
                notify(app, "Received a file from $from but could not save it to Downloads. It is kept inside Klip.", openKlip(app))
            }
        } catch (e: Throwable) {
            Log.w(TAG, "receiving failed: ${e.message}", e)
            runCatching { notify(app, "Received a file from $from but could not save it to Downloads. It is kept inside Klip.", openKlip(app)) }
        }
    }

    private fun copyToDownloads(ctx: Context, staged: File, name: String, rel: String): Boolean {
        val size = staged.length()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            val resolver = ctx.contentResolver
            val values = ContentValues().apply {
                put(MediaStore.MediaColumns.DISPLAY_NAME, name)
                put(MediaStore.MediaColumns.MIME_TYPE, FileText.mimeFor(name))
                put(MediaStore.MediaColumns.RELATIVE_PATH, "Download/Klip" + if (rel.isEmpty()) "" else "/$rel")
                put(MediaStore.MediaColumns.IS_PENDING, 1)
            }
            val uri = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values) ?: return false
            return try {
                resolver.openOutputStream(uri)?.use { out ->
                    staged.inputStream().use { it.copyTo(out, 256 * 1024) }
                } ?: throw java.io.IOException("cannot open the new file")
                val copied = resolver.openFileDescriptor(uri, "r")?.use { it.statSize } ?: -1L
                if (copied != size) throw java.io.IOException("copied $copied of $size bytes")
                values.clear()
                values.put(MediaStore.MediaColumns.IS_PENDING, 0)
                resolver.update(uri, values, null, null)
                true
            } catch (e: Throwable) {
                Log.w(TAG, "copy to Downloads failed: ${e.message}")
                runCatching { resolver.delete(uri, null, null) }
                false
            }
        }
        // Android 8 and 9: the app's own folder in external storage (no permission needed).
        val base = ctx.getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS) ?: return false
        val dir = File(File(base, "Klip"), rel).apply { mkdirs() }
        var target = File(dir, name)
        var n = 2
        while (target.exists()) {
            target = File(dir, name.substringBeforeLast('.', name) + " ($n)" +
                (name.substringAfterLast('.', "").let { if (it.isEmpty() || it == name) "" else ".$it" }))
            n++
        }
        return try {
            staged.copyTo(target)
            if (target.length() != size) { target.delete(); false } else true
        } catch (e: Throwable) {
            runCatching { target.delete() }
            false
        }
    }

    private fun pruneEmptyParents(ctx: Context, dir: File?) {
        val root = File(ctx.filesDir, "incoming")
        var d = dir
        while (d != null && d != root && d.path.startsWith(root.path) && d.list()?.isEmpty() == true) {
            d.delete()
            d = d.parentFile
        }
    }

    private fun downloads(ctx: Context): PendingIntent =
        PendingIntent.getActivity(
            ctx, 0, Intent(DownloadManager.ACTION_VIEW_DOWNLOADS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        )

    private fun openKlip(ctx: Context): PendingIntent =
        PendingIntent.getActivity(
            ctx, 1, Intent(ctx, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        )

    fun ensureChannel(ctx: Context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        ctx.getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(CHANNEL, "Files", NotificationManager.IMPORTANCE_DEFAULT).apply {
                description = "Tells you when a file from one of your devices has arrived"
                lockscreenVisibility = android.app.Notification.VISIBILITY_PRIVATE
            }
        )
    }

    private fun notify(ctx: Context, text: String, tap: PendingIntent, id: Int = nextNotification.getAndIncrement()) {
        ensureChannel(ctx)
        val n = NotificationCompat.Builder(ctx, CHANNEL)
            .setSmallIcon(R.drawable.ic_stat_klip)
            .setContentTitle("Klip")
            .setContentText(text)
            .setStyle(NotificationCompat.BigTextStyle().bigText(text))
            .setContentIntent(tap)
            .setAutoCancel(true)
            .setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
            .build()
        ctx.getSystemService(NotificationManager::class.java).notify(id, n)
    }

    /**
     * At service start: a file that could not be moved to Downloads (storage was full, the app was killed in the
     * middle of the copy) is tried again instead of waiting to be deleted. Files younger than two minutes are
     * left alone: they may be mid-copy right now.
     */
    fun retryStaged(ctx: Context) {
        val app = ctx.applicationContext
        val root = File(app.filesDir, "incoming")
        val now = System.currentTimeMillis()
        root.walkTopDown().filter { it.isFile && !(it.name.startsWith(".klip-") && it.name.endsWith(".part")) }
            .filter { now - it.lastModified() > 2 * 60_000 }
            .forEach { f ->
                val rel = f.parentFile?.relativeTo(root)?.path?.takeIf { it != "." } ?: ""
                onReceived(app, f.path, f.name, f.length(), "an earlier transfer", rel)
            }
    }

    /** Bytes kept inside Klip's own storage (staged files and partial transfers), for the settings screen. */
    fun keptBytes(ctx: Context): Long =
        File(ctx.filesDir, "incoming").walkTopDown().filter { it.isFile }.sumOf { it.length() }

    /** Called when the service starts: forget staged files nobody fetched for a week and old outgoing copies. */
    fun cleanStale(ctx: Context) {
        val now = System.currentTimeMillis()
        File(ctx.filesDir, "incoming").walkBottomUp().forEach {
            val name = it.name
            val ours = name.startsWith(".klip-") && name.endsWith(".part") // the engine sweeps its own partials
            if (it.isFile && !ours && now - it.lastModified() > 7L * 24 * 3600 * 1000) it.delete()
            else if (it.isDirectory && it.list()?.isEmpty() == true && it.name != "incoming") it.delete()
        }
        File(ctx.cacheDir, "outgoing").walkBottomUp().forEach {
            if (now - it.lastModified() > 24L * 3600 * 1000) it.delete()
        }
    }
}
