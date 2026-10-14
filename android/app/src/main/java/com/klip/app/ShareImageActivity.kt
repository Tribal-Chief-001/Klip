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
        // Read now: the permission the sharing app granted may end when this activity finishes.
        val raw = try {
            ImageSender.readBounded(this, uri)
        } catch (e: Throwable) {
            Toast.makeText(this, "Klip: ${e.message}", Toast.LENGTH_LONG).show()
            finish()
            return
        }
        SendHelper.sendImage(this, raw)
        finish()
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
