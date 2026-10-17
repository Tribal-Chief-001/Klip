package com.klip.app

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.provider.DocumentsContract
import android.util.Log
import android.widget.Toast
import uniffi.klip_android.FolderFile
import uniffi.klip_android.FolderSource
import uniffi.klip_android.TransferStateView

/**
 * Sends a folder the person picked with "Send a folder" (a document tree). The tree is listed once, then the engine asks
 * for each file's descriptor when that file's turn comes: no thousand open files, and the read permission of the tree is
 * kept (persisted) until the send is over, because a big folder takes longer than the picker's own grant lasts.
 */
object FolderSender {
    private const val TAG = "KlipFolderSender"

    fun start(ctx: Context, tree: Uri) {
        val app = ctx.applicationContext
        Thread { run(app, tree) }.start()
    }

    private fun run(app: Context, tree: Uri) {
        val resolver = app.contentResolver
        val persisted = runCatching {
            resolver.takePersistableUriPermission(tree, Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }.isSuccess
        try {
            val engine = KlipRuntime.engine
            if (engine == null) {
                KlipSyncService.start(app)
                toast(app, "Klip is starting — pick the folder again in a moment")
                return
            }
            val rootId = DocumentsContract.getTreeDocumentId(tree)
            val plan = FolderWalk.plan(rootId) { id -> children(app, tree, id) }
            if (plan.error != null) {
                toast(app, "Klip: ${plan.error}")
                return
            }
            val top = displayName(app, DocumentsContract.buildDocumentUriUsingTree(tree, rootId)) ?: "folder"
            val files = plan.files.map { FolderFile(it.rel, it.name, it.size.toULong()) }
            val source = object : FolderSource {
                override fun openFd(index: UInt): Int = try {
                    val doc = DocumentsContract.buildDocumentUriUsingTree(tree, plan.files[index.toInt()].id)
                    // detachFd: the engine owns the descriptor from here and closes it.
                    resolver.openFileDescriptor(doc, "r")?.detachFd() ?: -1
                } catch (e: Throwable) {
                    Log.w(TAG, "cannot open a file of the folder: ${e.message}")
                    -1
                }
            }
            try {
                val rows = engine.sendFolder(top, files, source)
                val started = rows.count { it.state != TransferStateView.FAILED }
                val skipped = if (plan.skipped > 0) " (${plan.skipped} skipped: cloud-only or unknown size)" else ""
                toast(app, "Klip: sending ${plan.files.size} files to ${rows.filter { it.state != TransferStateView.FAILED }.joinToString { it.peer }}$skipped")
                if (started > 0) waitUntilDone(engine)
            } catch (e: Throwable) {
                toast(app, "Klip: ${e.message}")
            }
        } finally {
            if (persisted) runCatching {
                resolver.releasePersistableUriPermission(tree, Intent.FLAG_GRANT_READ_URI_PERMISSION)
            }
        }
    }

    /** The grant must outlive the send: look once in a while until no outgoing transfer is left. */
    private fun waitUntilDone(engine: uniffi.klip_android.KlipEngine) {
        while (true) {
            Thread.sleep(2000)
            val running = engine.transfers().any {
                it.outgoing && (it.state == TransferStateView.WAITING || it.state == TransferStateView.ACTIVE)
            }
            if (!running) return
        }
    }

    private fun children(ctx: Context, tree: Uri, parentId: String): List<FolderWalk.Child> {
        val uri = DocumentsContract.buildChildDocumentsUriUsingTree(tree, parentId)
        val cols = arrayOf(
            DocumentsContract.Document.COLUMN_DOCUMENT_ID,
            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
            DocumentsContract.Document.COLUMN_MIME_TYPE,
            DocumentsContract.Document.COLUMN_SIZE,
            DocumentsContract.Document.COLUMN_FLAGS,
        )
        val out = ArrayList<FolderWalk.Child>()
        ctx.contentResolver.query(uri, cols, null, null, null)?.use { c ->
            while (c.moveToNext()) {
                val flags = c.getInt(4)
                out += FolderWalk.Child(
                    id = c.getString(0),
                    name = c.getString(1) ?: "file",
                    isDir = c.getString(2) == DocumentsContract.Document.MIME_TYPE_DIR,
                    size = if (c.isNull(3)) -1 else c.getLong(3),
                    virtual = flags and DocumentsContract.Document.FLAG_VIRTUAL_DOCUMENT != 0,
                )
            }
        }
        return out
    }

    private fun displayName(ctx: Context, doc: Uri): String? = try {
        ctx.contentResolver.query(doc, arrayOf(DocumentsContract.Document.COLUMN_DISPLAY_NAME), null, null, null)?.use {
            if (it.moveToFirst()) it.getString(0) else null
        }
    } catch (e: Throwable) {
        null
    }

    private fun toast(ctx: Context, msg: String) {
        Handler(Looper.getMainLooper()).post { Toast.makeText(ctx, msg, Toast.LENGTH_LONG).show() }
    }
}
