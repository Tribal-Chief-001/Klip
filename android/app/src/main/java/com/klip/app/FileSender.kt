package com.klip.app

import android.content.Context
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.os.ParcelFileDescriptor
import android.provider.OpenableColumns
import android.system.Os
import android.system.OsConstants
import android.util.Log
import android.widget.Toast
import java.io.File
import java.io.IOException
import java.util.UUID

/**
 * Sends files that another app shared with Klip. The Share sheet's permission to read a file can end when the
 * sharing activity finishes, so every file is opened first ([openAll]); the sends then run from the open
 * descriptors on a worker thread. The engine duplicates a descriptor, so ours are closed as soon as it returns.
 */
object FileSender {
    private const val TAG = "KlipFileSender"
    private const val MAX_FILES_PER_SHARE = 200
    /** The engine refuses more than 64 waiting files; stay well below so a big share never hits it. */
    private const val MAX_WAITING = 32

    class Item(val pfd: ParcelFileDescriptor, val name: String)

    /** Opens each file (quick, on the calling worker thread). Files that cannot be opened are counted, not fatal. */
    fun openAll(ctx: Context, uris: List<Uri>): Pair<List<Item>, Int> {
        val items = ArrayList<Item>()
        var failed = 0
        for (uri in uris.take(MAX_FILES_PER_SHARE)) {
            try {
                val pfd = ctx.contentResolver.openFileDescriptor(uri, "r") ?: throw IOException("cannot open")
                items += Item(pfd, FileText.safeName(displayName(ctx, uri)))
            } catch (e: Throwable) {
                Log.w(TAG, "cannot open a shared file: ${e.message}")
                failed++
            }
        }
        failed += (uris.size - MAX_FILES_PER_SHARE).coerceAtLeast(0)
        return items to failed
    }

    private fun displayName(ctx: Context, uri: Uri): String? =
        try {
            ctx.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use {
                if (it.moveToFirst()) it.getString(0) else null
            } ?: uri.lastPathSegment
        } catch (e: Throwable) {
            uri.lastPathSegment
        }

    /** Sends the opened files and tells the person how it went. Blocking: call from a worker thread. */
    fun sendAll(ctx: Context, items: List<Item>, unopened: Int) {
        val app = ctx.applicationContext
        val engine = KlipRuntime.engine
        if (engine == null) {
            items.forEach { runCatching { it.pfd.close() } }
            KlipSyncService.start(app)
            toast(app, "Klip is starting — share the file again in a moment")
            return
        }
        val devices = LinkedHashSet<String>()
        val errors = ArrayList<String>()
        var started = 0
        for (item in items) {
            try {
                waitForRoom(engine)
                val views = sendOne(app, engine, item)
                started += views.count { it.state != uniffi.klip_android.TransferStateView.FAILED }
                views.forEach { v ->
                    if (v.state == uniffi.klip_android.TransferStateView.FAILED) errors += (v.error ?: "failed") else devices += v.peer
                }
            } catch (e: Throwable) {
                errors += (e.message ?: "failed")
            } finally {
                runCatching { item.pfd.close() }
            }
        }
        if (unopened > 0) errors += "$unopened file(s) could not be opened"
        toast(app, summary(started, devices, errors.distinct()))
    }

    private fun isRegular(pfd: ParcelFileDescriptor): Boolean =
        try { OsConstants.S_ISREG(Os.fstat(pfd.fileDescriptor).st_mode) } catch (e: Throwable) { false }

    private fun sendOne(app: Context, engine: uniffi.klip_android.KlipEngine, item: Item): List<uniffi.klip_android.TransferView> {
        if (isRegular(item.pfd)) return engine.sendFileFd(item.pfd.fd, item.name)
        // A pipe from a cloud provider cannot be read twice or measured: copy it into the cache first.
        val copy = copyToCache(app, item)
        try {
            return ParcelFileDescriptor.open(copy, ParcelFileDescriptor.MODE_READ_ONLY).use {
                engine.sendFileFd(it.fd, item.name)
            }
        } finally {
            // The engine works on its own duplicate: deleting the name now is safe on Linux.
            copy.delete()
            copy.parentFile?.delete()
        }
    }

    private fun copyToCache(app: Context, item: Item): File {
        val dir = File(File(app.cacheDir, "outgoing"), UUID.randomUUID().toString()).apply { mkdirs() }
        val target = File(dir, item.name)
        val limit = KlipRuntime.maxFileMb(app).toLong() * 1024 * 1024
        try {
            ParcelFileDescriptor.AutoCloseInputStream(item.pfd).use { input ->
                target.outputStream().use { out ->
                    val buf = ByteArray(256 * 1024)
                    var total = 0L
                    while (true) {
                        val n = input.read(buf)
                        if (n < 0) break
                        total += n
                        if (total > limit) throw IOException("the file is bigger than the limit (${FileText.size(limit)})")
                        if (app.cacheDir.usableSpace < 64L * 1024 * 1024) throw IOException("not enough free space to copy the file")
                        out.write(buf, 0, n)
                    }
                }
            }
        } catch (e: Throwable) {
            target.delete(); dir.delete()
            throw e
        }
        return target
    }

    private fun waitForRoom(engine: uniffi.klip_android.KlipEngine) {
        val end = System.currentTimeMillis() + 10 * 60_000
        while (engine.transfers().count { it.state == uniffi.klip_android.TransferStateView.WAITING } >= MAX_WAITING) {
            if (System.currentTimeMillis() > end) throw IOException("too many files are waiting; try fewer at a time")
            Thread.sleep(500)
        }
    }

    private fun summary(started: Int, devices: Set<String>, errors: List<String>): String = when {
        started > 0 && errors.isEmpty() -> "Klip: sending ${if (started == 1) "1 file" else "$started files"} to ${devices.joinToString()}"
        started > 0 -> "Klip: sending ${if (started == 1) "1 file" else "$started files"} to ${devices.joinToString()} — problem: ${errors.first()}"
        else -> "Klip: " + (errors.firstOrNull() ?: "no linked devices yet")
    }

    private fun toast(ctx: Context, msg: String) {
        Handler(Looper.getMainLooper()).post { Toast.makeText(ctx, msg, Toast.LENGTH_LONG).show() }
    }
}
