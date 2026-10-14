package com.klip.app

import android.content.Context
import android.net.Uri
import androidx.core.content.FileProvider
import java.io.File
import java.io.IOException

/**
 * Images received from other devices. The clipboard holds only a content URI that points at a file here,
 * so Android's ~1 MB Binder limit for clip data does not apply. Files are short-lived: old ones are
 * deleted on every write and when the service starts.
 */
object ClipImages {
    private const val KEEP = 4
    private const val MAX_AGE_MS = 60 * 60 * 1000L

    private fun dir(ctx: Context): File = File(ctx.applicationContext.cacheDir, "clips").apply { mkdirs() }

    /** Delete files older than an hour, and all but the newest [KEEP]. */
    fun prune(ctx: Context, now: Long = System.currentTimeMillis()) {
        val files = dir(ctx).listFiles()?.filter { it.isFile }?.sortedByDescending { it.lastModified() } ?: return
        files.forEachIndexed { i, f -> if (i >= KEEP || now - f.lastModified() > MAX_AGE_MS) f.delete() }
    }

    /** Store [data] and return a URI other apps can read through the clipboard. */
    fun store(ctx: Context, data: ByteArray, mime: String): Uri {
        val app = ctx.applicationContext
        val d = dir(app)
        val ext = if (mime == "image/webp") "webp" else "png"
        val name = "klip-${System.currentTimeMillis()}-${System.nanoTime() % 100000}"
        val tmp = File(d, ".$name.tmp")
        val file = File(d, "$name.$ext")
        try {
            tmp.writeBytes(data)
            if (!tmp.renameTo(file)) throw IOException("could not store the image")
        } catch (e: Throwable) {
            tmp.delete()
            throw e
        }
        prune(app)
        return FileProvider.getUriForFile(app, app.packageName + ".clips", file)
    }
}
