package com.klip.app

import android.app.Activity
import android.content.Intent
import android.os.Bundle

/** "Klip" in the system Share sheet: share a link or text from any app to your other devices. */
class ShareTextActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val text = intent.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString()
        if (!text.isNullOrEmpty()) SendHelper.sendText(this, text)
        finish()
    }
}
