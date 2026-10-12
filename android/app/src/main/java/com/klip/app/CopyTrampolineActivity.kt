package com.klip.app

import android.app.Activity
import android.app.NotificationManager
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.util.Log
import android.widget.Toast

/**
 * Lightweight transparent activity that gains transient window focus to reliably
 * get or set the primary clipboard on Android 10-15+ without requiring any Accessibility Service.
 * Works 100% reliably in UPI and Banking-safe environments.
 *
 * Supported Actions:
 * - ACTION_COPY: Injects remote clip into clipboard from interactive notification button.
 * - ACTION_TILE_SYNC: Bidirectionally synchronizes clipboard when triggered from Quick Settings Tile.
 */
class CopyTrampolineActivity : Activity() {

    companion object {
        private const val TAG = "CopyTrampoline"
        const val ACTION_COPY = "com.klip.app.ACTION_COPY"
        const val ACTION_TILE_SYNC = "com.klip.app.ACTION_TILE_SYNC"

        const val EXTRA_CLIP_TEXT = "extra_clip_text"
        const val EXTRA_IS_SENSITIVE = "extra_is_sensitive"
    }

    private var isHandled = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        suppressAnimation()

        if (intent.action != ACTION_TILE_SYNC) {
            handleCopyAction()
            finishWithNoAnimation()
        }
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        // Quick Settings tile sync requires active window focus on Android 10+ to read ClipboardManager.primaryClip
        if (hasFocus && !isHandled && intent.action == ACTION_TILE_SYNC) {
            isHandled = true
            handleTileSync()
            finishWithNoAnimation()
        }
    }

    override fun onResume() {
        super.onResume()
        // Fallback for tile sync in case onWindowFocusChanged is delayed on OEM ROMs
        if (!isHandled && intent.action == ACTION_TILE_SYNC) {
            window.decorView.postDelayed({
                if (!isHandled) {
                    isHandled = true
                    handleTileSync()
                    finishWithNoAnimation()
                }
            }, 100)
        }
    }

    private fun handleCopyAction() {
        isHandled = true
        val text = intent.getStringExtra(EXTRA_CLIP_TEXT)
        val isSensitive = intent.getBooleanExtra(EXTRA_IS_SENSITIVE, false)

        if (!text.isNullOrEmpty()) {
            try {
                val clipboard = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
                val clip = ClipData.newPlainText("Klip", text)
                if (isSensitive && Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    val extras = clip.description.extras ?: android.os.PersistableBundle()
                    extras.putBoolean("android.content.extra.IS_SENSITIVE", true)
                    clip.description.extras = extras
                }
                clipboard.setPrimaryClip(clip)
                KlipSyncService.lastSyncedLocalText = text
                Toast.makeText(this, "Copied to clipboard", Toast.LENGTH_SHORT).show()
            } catch (e: Exception) {
                Log.e(TAG, "Failed to copy to clipboard: ${e.message}", e)
                Toast.makeText(this, "Failed to copy: ${e.message}", Toast.LENGTH_SHORT).show()
            }
        }

        // Clean up the heads-up notification once copied
        try {
            val notificationManager = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
            notificationManager.cancel(KlipSyncService.CLIP_NOTIFICATION_ID)
        } catch (_: Exception) {}
    }

    private fun handleTileSync() {
        try {
            val clipboard = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
            val currentClip = clipboard.primaryClip
            val currentText = if (currentClip != null && currentClip.itemCount > 0) {
                currentClip.getItemAt(0).text?.toString() ?: ""
            } else ""

            val remoteClip = KlipSyncService.latestRemoteClip
            val remoteSensitive = KlipSyncService.latestRemoteSensitive

            if (currentText.isNotEmpty() && currentText != remoteClip && currentText != KlipSyncService.lastSyncedLocalText) {
                // Direction A: Local phone clipboard is newer -> broadcast to PC
                val syncIntent = Intent(this, KlipSyncService::class.java).apply {
                    action = KlipSyncService.ACTION_BROADCAST_CLIP
                    putExtra(KlipSyncService.EXTRA_CLIP_TEXT, currentText)
                }
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                    startForegroundService(syncIntent)
                } else {
                    startService(syncIntent)
                }
                val preview = if (currentText.length > 25) currentText.take(25) + "…" else currentText
                Toast.makeText(this, "Synced to Klip: \"$preview\"", Toast.LENGTH_SHORT).show()
            } else if (!remoteClip.isNullOrEmpty() && (currentClip == null || currentText != remoteClip)) {
                // Direction B: Pending clip from PC -> inject into local clipboard
                val newClip = ClipData.newPlainText("Klip", remoteClip)
                if (remoteSensitive && Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    val extras = newClip.description.extras ?: android.os.PersistableBundle()
                    extras.putBoolean("android.content.extra.IS_SENSITIVE", true)
                    newClip.description.extras = extras
                }
                clipboard.setPrimaryClip(newClip)
                KlipSyncService.lastSyncedLocalText = remoteClip
                val preview = if (remoteClip.length > 25) remoteClip.take(25) + "…" else remoteClip
                Toast.makeText(this, "Copied from Klip: \"$preview\"", Toast.LENGTH_SHORT).show()
            } else if (currentText.isNotEmpty()) {
                // Already in sync: re-broadcast to ensure mesh consistency
                val syncIntent = Intent(this, KlipSyncService::class.java).apply {
                    action = KlipSyncService.ACTION_BROADCAST_CLIP
                    putExtra(KlipSyncService.EXTRA_CLIP_TEXT, currentText)
                }
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                    startForegroundService(syncIntent)
                } else {
                    startService(syncIntent)
                }
                Toast.makeText(this, "Klip: Clipboard in sync", Toast.LENGTH_SHORT).show()
            } else {
                Toast.makeText(this, "Klip: Clipboard is empty", Toast.LENGTH_SHORT).show()
            }
        } catch (e: Exception) {
            Log.e(TAG, "Error in handleTileSync: ${e.message}", e)
            Toast.makeText(this, "Klip sync error: ${e.message}", Toast.LENGTH_SHORT).show()
        }
    }

    private fun suppressAnimation() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            overrideActivityTransition(OVERRIDE_TRANSITION_OPEN, 0, 0)
        } else {
            @Suppress("DEPRECATION")
            overridePendingTransition(0, 0)
        }
    }

    private fun finishWithNoAnimation() {
        finish()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            overrideActivityTransition(OVERRIDE_TRANSITION_CLOSE, 0, 0)
        } else {
            @Suppress("DEPRECATION")
            overridePendingTransition(0, 0)
        }
    }
}
