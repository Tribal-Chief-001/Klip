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
        }

        finish()
    }
}
