package com.klip.app.debug

import android.database.Cursor
import android.database.MatrixCursor
import android.os.CancellationSignal
import android.os.ParcelFileDescriptor
import android.provider.DocumentsContract.Document
import android.provider.DocumentsContract.Root
import android.provider.DocumentsProvider
import java.io.File

/**
 * Debug only: a small document tree to send as a folder.
 *   Trip/ (root)  a.txt, Day 1/{b.bin (300 KB), raw/c.cr2 (1 MB)}, Day 2/{d.jpg}, "Cloud doc" (a virtual document)
 */
class DebugTreeProvider : DocumentsProvider() {
    private data class Doc(val id: String, val name: String, val dir: Boolean, val size: Long, val virtual: Boolean = false, val children: List<String> = emptyList())

    private val docs: Map<String, Doc> = listOf(
        Doc("root", "Trip", true, 0, children = listOf("a", "d1", "d2", "cloud")),
        Doc("a", "a.txt", false, 1234),
        Doc("d1", "Day 1", true, 0, children = listOf("b", "raw")),
        Doc("b", "b.bin", false, 300_000),
        Doc("raw", "raw", true, 0, children = listOf("c")),
        Doc("c", "c.cr2", false, 1_000_000),
        Doc("d2", "Day 2", true, 0, children = listOf("d")),
        Doc("d", "d.jpg", false, 50_000),
        Doc("cloud", "Cloud doc", false, -1, virtual = true),
    ).associateBy { it.id }

    override fun onCreate() = true

    private fun row(c: MatrixCursor, d: Doc) {
        var flags = 0
        if (d.virtual) flags = flags or Document.FLAG_VIRTUAL_DOCUMENT
        c.newRow()
            .add(Document.COLUMN_DOCUMENT_ID, d.id)
            .add(Document.COLUMN_DISPLAY_NAME, d.name)
            .add(Document.COLUMN_MIME_TYPE, if (d.dir) Document.MIME_TYPE_DIR else "application/octet-stream")
            .add(Document.COLUMN_SIZE, if (d.size < 0) null else d.size)
            .add(Document.COLUMN_FLAGS, flags)
            .add(Document.COLUMN_LAST_MODIFIED, 0L)
    }

    private val docColumns = arrayOf(
        Document.COLUMN_DOCUMENT_ID, Document.COLUMN_DISPLAY_NAME, Document.COLUMN_MIME_TYPE,
        Document.COLUMN_SIZE, Document.COLUMN_FLAGS, Document.COLUMN_LAST_MODIFIED,
    )

    override fun queryRoots(projection: Array<out String>?): Cursor {
        val c = MatrixCursor(arrayOf(Root.COLUMN_ROOT_ID, Root.COLUMN_TITLE, Root.COLUMN_DOCUMENT_ID, Root.COLUMN_FLAGS, Root.COLUMN_ICON))
        c.newRow().add(Root.COLUMN_ROOT_ID, "debug").add(Root.COLUMN_TITLE, "Klip debug tree").add(Root.COLUMN_DOCUMENT_ID, "root")
            .add(Root.COLUMN_FLAGS, 0).add(Root.COLUMN_ICON, android.R.drawable.ic_menu_gallery)
        return c
    }

    override fun queryDocument(documentId: String, projection: Array<out String>?): Cursor {
        val c = MatrixCursor(docColumns)
        docs[documentId]?.let { row(c, it) }
        return c
    }

    override fun queryChildDocuments(parentDocumentId: String, projection: Array<out String>?, sortOrder: String?): Cursor {
        val c = MatrixCursor(docColumns)
        docs[parentDocumentId]?.children?.forEach { id -> docs[id]?.let { row(c, it) } }
        return c
    }

    override fun openDocument(documentId: String, mode: String, signal: CancellationSignal?): ParcelFileDescriptor {
        val d = docs[documentId] ?: throw java.io.FileNotFoundException(documentId)
        val bytes = ByteArray(d.size.toInt()) { ((it * 17 + d.id.hashCode()) % 251).toByte() }
        if (d.id == "d") {
            // d.jpg comes through a pipe, like a cloud provider's file: no size, no seek.
            val (read, write) = ParcelFileDescriptor.createPipe()
            Thread { ParcelFileDescriptor.AutoCloseOutputStream(write).use { it.write(bytes) } }.start()
            return read
        }
        val f = File(context!!.cacheDir, "debug-tree-${d.id}.bin")
        if (!f.exists() || f.length() != d.size) f.writeBytes(bytes)
        return ParcelFileDescriptor.open(f, ParcelFileDescriptor.MODE_READ_ONLY)
    }

    /** A document tree requires "is a descendant of", not only "is a direct child of". */
    override fun isChildDocument(parentDocumentId: String, documentId: String): Boolean {
        val kids = docs[parentDocumentId]?.children ?: return false
        return documentId in kids || kids.any { isChildDocument(it, documentId) }
    }
}
