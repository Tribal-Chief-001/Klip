package com.klip.app.debug

import android.content.ContentProvider
import android.content.ContentValues
import android.database.Cursor
import android.database.MatrixCursor
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.OpenableColumns
import java.io.File

/**
 * Debug only. `content://<app>.debugfiles/file/<n>` is a real file of 100,000 + n bytes (every byte derived from n);
 * `content://<app>.debugfiles/pipe/<n>` has the same bytes but comes through a pipe, with no size and no seek, the way a
 * cloud provider hands them out.
 */
class DebugFilesProvider : ContentProvider() {
    override fun onCreate() = true

    private fun parse(uri: Uri): Pair<String, Int>? {
        val seg = uri.pathSegments
        if (seg.size != 2) return null
        return seg[0] to (seg[1].toIntOrNull() ?: return null)
    }

    private fun bytes(n: Int) = ByteArray(100_000 + n) { ((it * 31 + n) % 251).toByte() }

    override fun query(uri: Uri, projection: Array<out String>?, s: String?, a: Array<out String>?, o: String?): Cursor? {
        val (kind, n) = parse(uri) ?: return null
        val cursor = MatrixCursor(arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE))
        cursor.addRow(arrayOf<Any?>("debug-$kind-$n.bin", if (kind == "pipe") null else (100_000 + n).toLong()))
        return cursor
    }

    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor? {
        val (kind, n) = parse(uri) ?: return null
        return if (kind == "file") {
            val f = File(context!!.cacheDir, "debug-file-$n.bin")
            if (!f.exists() || f.length() != (100_000 + n).toLong()) f.writeBytes(bytes(n))
            ParcelFileDescriptor.open(f, ParcelFileDescriptor.MODE_READ_ONLY)
        } else {
            val (read, write) = ParcelFileDescriptor.createPipe()
            Thread {
                ParcelFileDescriptor.AutoCloseOutputStream(write).use { it.write(bytes(n)) }
            }.start()
            read
        }
    }

    override fun getType(uri: Uri) = "application/octet-stream"
    override fun insert(uri: Uri, values: ContentValues?): Uri? = null
    override fun delete(uri: Uri, s: String?, a: Array<out String>?) = 0
    override fun update(uri: Uri, v: ContentValues?, s: String?, a: Array<out String>?) = 0
}
