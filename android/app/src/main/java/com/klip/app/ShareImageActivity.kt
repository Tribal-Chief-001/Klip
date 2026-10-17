package com.klip.app

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.widget.Toast

/** "Klip" in the Share sheet for images (one image; more are ignored). */
class ShareImageActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val uri = streamUri()
        if (uri == null) {
            Toast.makeText(this, "Klip: no image received", Toast.LENGTH_SHORT).show()
            finish()
            return
        }
        // Read on a worker: a slow (cloud-backed) source must not freeze the screen. The sharing app's permission
        // to read may end when this activity finishes, so it stays (invisibly) until the bytes are read.
        val app = applicationContext
        Thread {
            val raw = try {
                ImageSender.readBounded(app, uri)
            } catch (e: Throwable) {
                runOnUiThread {
                    Toast.makeText(app, "Klip: ${e.message}", Toast.LENGTH_LONG).show()
                    finish()
                }
                return@Thread
            }
            runOnUiThread { finish() }
            SendHelper.sendImage(app, raw)
        }.start()
    }

    private fun streamUri(): Uri? {
        val fromExtra = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
        } else {
            @Suppress("DEPRECATION")
            intent.getParcelableExtra<Uri>(Intent.EXTRA_STREAM)
        }
        return fromExtra ?: intent.clipData?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.uri
    }
}
