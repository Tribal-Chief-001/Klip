package com.klip.app

import android.content.Context
import android.os.ParcelFileDescriptor
import android.system.Os
import android.system.OsConstants
import java.io.File
import java.io.IOException
import java.util.UUID

/** A file a document provider hands out as a pipe (no size, no seek) cannot be sent as it is: it is copied to the cache first. */
object PipeCopy {
    fun isRegular(pfd: ParcelFileDescriptor): Boolean =
        try { OsConstants.S_ISREG(Os.fstat(pfd.fileDescriptor).st_mode) } catch (e: Throwable) { false }

    /** Copy everything [pfd] delivers into `cache/outgoing/<random>/<name>`, at most [limitBytes], leaving room on the disk. */
    fun copyToCache(app: Context, pfd: ParcelFileDescriptor, name: String, limitBytes: Long): File {
        val dir = File(File(app.cacheDir, "outgoing"), UUID.randomUUID().toString()).apply { mkdirs() }
        val target = File(dir, FileText.safeName(name))
        try {
            ParcelFileDescriptor.AutoCloseInputStream(pfd).use { input ->
                target.outputStream().use { out ->
                    val buf = ByteArray(256 * 1024)
                    var total = 0L
                    while (true) {
                        val n = input.read(buf)
                        if (n < 0) break
                        total += n
                        if (total > limitBytes) throw IOException("the file is bigger than the limit (${FileText.size(limitBytes)})")
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

    /** An open descriptor of the cache copy that outlives the file's name (the name is deleted at once). */
    fun openAndForget(copy: File): ParcelFileDescriptor {
        val pfd = ParcelFileDescriptor.open(copy, ParcelFileDescriptor.MODE_READ_ONLY)
        copy.delete()
        copy.parentFile?.delete()
        return pfd
    }
}
