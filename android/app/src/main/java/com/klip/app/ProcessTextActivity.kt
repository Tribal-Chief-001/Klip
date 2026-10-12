package com.klip.app

import android.app.Activity
import android.content.Intent
import android.os.Bundle
import android.widget.Toast

/** "Send to Klip" in the text selection menu of any app. Needs no special permission. */
class ProcessTextActivity : Activity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val text = intent.getCharSequenceExtra(Intent.EXTRA_PROCESS_TEXT)?.toString()
        val app = applicationContext

        if (text.isNullOrEmpty()) {
            setResult(RESULT_CANCELED)
        } else {
            val engine = KlipRuntime.engine
            if (engine == null) {
                KlipSyncService.start(app)
                Toast.makeText(app, "Klip is starting — select the text again in a moment", Toast.LENGTH_LONG).show()
            } else {
                Thread {
                    val msg = try {
                        val r = engine.sendText(text)
                        when {
                            r.delivered.isNotEmpty() -> "Klip: sent to ${r.delivered.joinToString()}"
                            r.failed.isNotEmpty() -> "Klip: not delivered — " + r.failed.joinToString { "${it.device}: ${it.reason}" }
                            else -> "Klip: no linked devices yet"
                        }
                    } catch (e: Throwable) {
                        "Klip: ${e.message}"
                    }
                    android.os.Handler(android.os.Looper.getMainLooper()).post {
                        Toast.makeText(app, msg, Toast.LENGTH_LONG).show()
                    }
                }.start()
            }
            // Return the text unchanged so the calling editor keeps its selection.
            setResult(RESULT_OK, Intent().putExtra(Intent.EXTRA_PROCESS_TEXT, text))
        }
        finish()
    }
}
