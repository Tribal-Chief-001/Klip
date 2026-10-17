package com.klip.app

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.widget.Toast

/**
 * "Send as a file with Klip" in the Share sheet, for one file or several. The activity stays (invisibly) only
 * until every shared file is open: that is when the sharing app's permission to read them is still certain.
 */
class ShareFileActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val uris = streamUris()
        if (uris.isEmpty()) {
            Toast.makeText(this, "Klip: no file received", Toast.LENGTH_SHORT).show()
            finish()
            return
        }
        val app = applicationContext
        Thread {
            val (items, unopened) = FileSender.openAll(app, uris)
            runOnUiThread { finish() } // every file is open now: the permission is no longer needed
            FileSender.sendAll(app, items, unopened)
        }.start()
    }

    private fun streamUris(): List<Uri> {
        val out = LinkedHashSet<Uri>()
        if (intent.action == Intent.ACTION_SEND_MULTIPLE) {
            val list = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java)
            } else {
                @Suppress("DEPRECATION")
                intent.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM)
            }
            list?.let { out.addAll(it) }
        } else {
            val one = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
            } else {
                @Suppress("DEPRECATION")
                intent.getParcelableExtra<Uri>(Intent.EXTRA_STREAM)
            }
            one?.let { out.add(it) }
        }
        intent.clipData?.let { clip -> for (i in 0 until clip.itemCount) clip.getItemAt(i).uri?.let { out.add(it) } }
        return out.toList()
    }
}
