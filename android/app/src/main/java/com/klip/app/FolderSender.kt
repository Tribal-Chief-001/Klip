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

    /** Two sends of the same tree must not release each other's persisted grant: count them. */
    private val grants = HashMap<String, Int>()

    @Synchronized
    private fun holdGrant(resolver: android.content.ContentResolver, tree: Uri): Boolean {
        val key = tree.toString()
        val n = grants[key] ?: 0
        val ok = n > 0 || runCatching {
            resolver.takePersistableUriPermission(tree, Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }.isSuccess
        if (ok) grants[key] = n + 1
        return ok
    }

    @Synchronized
    private fun dropGrant(resolver: android.content.ContentResolver, tree: Uri) {
        val key = tree.toString()
        val n = (grants[key] ?: return) - 1
        if (n > 0) {
            grants[key] = n
        } else {
            grants.remove(key)
            runCatching { resolver.releasePersistableUriPermission(tree, Intent.FLAG_GRANT_READ_URI_PERMISSION) }
        }
    }

    fun start(ctx: Context, tree: Uri) {
        val app = ctx.applicationContext
        Thread {
            try {
                run(app, tree)
            } catch (e: Throwable) {
                // A provider may throw anything (a revoked grant, a closed cloud account): never take the app down.
                Log.w(TAG, "cannot send the folder: ${e.message}", e)
                toast(app, "Klip: cannot read the folder (${e.message ?: e.javaClass.simpleName})")
            }
        }.start()
    }

    private fun run(app: Context, tree: Uri) {
        val resolver = app.contentResolver
        val persisted = holdGrant(resolver, tree)
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
                    val pfd = resolver.openFileDescriptor(doc, "r")
                    when {
                        pfd == null -> -1
                        // detachFd: the engine owns the descriptor from here and closes it.
                        PipeCopy.isRegular(pfd) -> pfd.detachFd()
                        // A provider that serves a file as a pipe: copy it to the cache, hand over the copy.
                        else -> PipeCopy.openAndForget(
                            PipeCopy.copyToCache(app, pfd, plan.files[index.toInt()].name, KlipRuntime.maxFileMb(app).toLong() * 1024 * 1024)
                        ).detachFd()
                    }
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
            if (persisted) dropGrant(resolver, tree)
        }
    }

    /** The grant must outlive the send: look once in a while until no outgoing transfer is left. */
    private fun waitUntilDone(engine: uniffi.klip_android.KlipEngine) {
        while (true) {
            Thread.sleep(2000)
            // The engine was stopped or replaced meanwhile: nothing is left to wait for.
            if (KlipRuntime.engine !== engine) return
            val running = try {
                engine.transfers().any {
                    it.outgoing && (it.state == TransferStateView.WAITING || it.state == TransferStateView.ACTIVE)
                }
            } catch (e: Throwable) {
                false
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
            // A provider may return more columns, in another order, than were asked for: read them by name.
            val id = c.getColumnIndexOrThrow(DocumentsContract.Document.COLUMN_DOCUMENT_ID)
            val name = c.getColumnIndexOrThrow(DocumentsContract.Document.COLUMN_DISPLAY_NAME)
            val mime = c.getColumnIndexOrThrow(DocumentsContract.Document.COLUMN_MIME_TYPE)
            val size = c.getColumnIndexOrThrow(DocumentsContract.Document.COLUMN_SIZE)
            val flagsAt = c.getColumnIndexOrThrow(DocumentsContract.Document.COLUMN_FLAGS)
            while (c.moveToNext()) {
                val flags = c.getInt(flagsAt)
                out += FolderWalk.Child(
                    id = c.getString(id),
                    name = c.getString(name) ?: "file",
                    isDir = c.getString(mime) == DocumentsContract.Document.MIME_TYPE_DIR,
                    size = if (c.isNull(size)) -1 else c.getLong(size),
                    virtual = flags and DocumentsContract.Document.FLAG_VIRTUAL_DOCUMENT != 0,
                )
            }
        }
        return out
    }

    private fun displayName(ctx: Context, doc: Uri): String? = try {
        ctx.contentResolver.query(doc, arrayOf(DocumentsContract.Document.COLUMN_DISPLAY_NAME), null, null, null)?.use {
            val at = it.getColumnIndex(DocumentsContract.Document.COLUMN_DISPLAY_NAME)
            if (at >= 0 && it.moveToFirst()) it.getString(at) else null
        }
    } catch (e: Throwable) {
        null
    }

    private fun toast(ctx: Context, msg: String) {
        Handler(Looper.getMainLooper()).post { Toast.makeText(ctx, msg, Toast.LENGTH_LONG).show() }
    }
}
