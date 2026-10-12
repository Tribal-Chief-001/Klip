package com.klip.app

import android.app.Activity
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.os.Build
import android.os.Bundle
import android.widget.Toast

/**
 * Lightweight transparent activity that gains transient window focus to reliably
 * set the primary clipboard on Android 10-15+ without requiring any Accessibility Service.
 * Works 100% reliably in UPI and Banking-safe environments.
 */
class CopyTrampolineActivity : Activity() {

    companion object {
        const val EXTRA_CLIP_TEXT = "extra_clip_text"
        const val EXTRA_IS_SENSITIVE = "extra_is_sensitive"
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        handleIntent()
    }

    private fun handleIntent() {
        val text = intent.getStringExtra(EXTRA_CLIP_TEXT)
        val isSensitive = intent.getBooleanExtra(EXTRA_IS_SENSITIVE, false)

        if (!text.isNullOrEmpty()) {
            try {
                val clipboard = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
                val clip = ClipData.newPlainText("Klip", text)
                if (isSensitive && Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    clip.description.extras = (clip.description.extras ?: android.os.PersistableBundle()).apply {
                        putBoolean("android.content.extra.IS_SENSITIVE", true)
                    }
                }
                clipboard.setPrimaryClip(clip)
                Toast.makeText(this, "Copied to clipboard", Toast.LENGTH_SHORT).show()
            } catch (e: Exception) {
                Toast.makeText(this, "Failed to copy: ${e.message}", Toast.LENGTH_SHORT).show()
            }
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
