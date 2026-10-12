package com.klip.app

import android.app.Activity
import android.content.Intent
import android.os.Bundle

/** "Send to Klip" in the text selection menu of any app. Needs no special permission. */
class ProcessTextActivity : Activity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val text = intent.getCharSequenceExtra(Intent.EXTRA_PROCESS_TEXT)?.toString()

        if (text.isNullOrEmpty()) {
            setResult(RESULT_CANCELED)
        } else {
            SendHelper.sendText(this, text)
            // Return the text unchanged so the calling editor keeps its selection.
            setResult(RESULT_OK, Intent().putExtra(Intent.EXTRA_PROCESS_TEXT, text))
        }
        finish()
    }
}
