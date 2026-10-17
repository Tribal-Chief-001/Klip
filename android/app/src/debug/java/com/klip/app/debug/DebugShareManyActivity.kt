package com.klip.app.debug

import android.app.Activity
import android.content.ClipData
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import com.klip.app.ShareFileActivity

/**
 * Debug only: `adb shell am start -n com.klip.app/.debug.DebugShareManyActivity --ei count 40 --ei pipes 2` shares `count`
 * files and `pipes` pipe-backed files to Klip with ACTION_SEND_MULTIPLE, as another app's "Share" would.
 */
class DebugShareManyActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val count = intent.getIntExtra("count", 40)
        val pipes = intent.getIntExtra("pipes", 0)
        val auth = "$packageName.debugfiles"
        val uris = ArrayList<Uri>()
        for (i in 0 until count) uris += Uri.parse("content://$auth/file/$i")
        for (i in 0 until pipes) uris += Uri.parse("content://$auth/pipe/${1000 + i}")
        val share = Intent(Intent.ACTION_SEND_MULTIPLE).apply {
            setClass(this@DebugShareManyActivity, ShareFileActivity::class.java)
            type = "application/octet-stream"
            putParcelableArrayListExtra(Intent.EXTRA_STREAM, uris)
            clipData = ClipData.newRawUri("files", uris.first()).also { c -> uris.drop(1).forEach { c.addItem(ClipData.Item(it)) } }
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
        startActivity(share)
        finish()
    }
}
