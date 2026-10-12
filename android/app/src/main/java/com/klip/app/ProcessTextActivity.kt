package com.klip.app

import android.app.Activity
import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.widget.Toast

/**
 * Direct Text Selection Action (PROCESS_TEXT).
 * When text is selected in any Android app (Chrome, WhatsApp, Notes, etc.),
 * "Send to Klip" appears in the system context menu, instantly broadcasting
 * the selected snippet to PC peers with zero special permissions.
 */
class ProcessTextActivity : Activity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            overrideActivityTransition(OVERRIDE_TRANSITION_OPEN, 0, 0)
        } else {
            @Suppress("DEPRECATION")
            overridePendingTransition(0, 0)
        }

        val selectedText = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            intent.getCharSequenceExtra(Intent.EXTRA_PROCESS_TEXT)?.toString()
        } else {
            intent.getStringExtra(Intent.EXTRA_TEXT)
        }

        if (!selectedText.isNullOrEmpty()) {
            val syncIntent = Intent(this, KlipSyncService::class.java).apply {
                action = KlipSyncService.ACTION_BROADCAST_CLIP
                putExtra(KlipSyncService.EXTRA_CLIP_TEXT, selectedText)
            }
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                startForegroundService(syncIntent)
            } else {
                startService(syncIntent)
            }
            val preview = if (selectedText.length > 25) selectedText.take(25) + "…" else selectedText
            Toast.makeText(this, "Sent to Klip: \"$preview\"", Toast.LENGTH_SHORT).show()

            // Return original text so calling editor preserves selection without modifying it
            val resultIntent = Intent().putExtra(Intent.EXTRA_PROCESS_TEXT, selectedText)
            setResult(RESULT_OK, resultIntent)
        } else {
            setResult(RESULT_CANCELED)
        }

        finish()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            overrideActivityTransition(OVERRIDE_TRANSITION_CLOSE, 0, 0)
        } else {
            @Suppress("DEPRECATION")
            overridePendingTransition(0, 0)
        }
    }
}
