package com.klip.app

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.os.Build
import android.service.quicksettings.Tile
import android.service.quicksettings.TileService
import android.util.Log
import android.widget.Toast

/**
 * Android Quick Settings (QS) Tile Service.
 * Allows users to pull down the Android status bar and tap the "📎 Klip" tile
 * to instantly sync clipboard bidirectionally with active window focus,
 * 100% safe from all banking/UPI restrictions.
 */
class KlipTileService : TileService() {

    companion object {
        private const val TAG = "KlipTileService"
    }

    override fun onStartListening() {
        super.onStartListening()
        qsTile?.let { tile ->
            tile.state = Tile.STATE_ACTIVE
            tile.label = "Klip Sync"
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                tile.subtitle = "Tap to sync clip"
            }
            tile.updateTile()
        }
    }

    override fun onClick() {
        super.onClick()
        Log.i(TAG, "Quick Settings Klip tile tapped")

        try {
            val clipboard = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
            val clip = clipboard.primaryClip

            if (clip != null && clip.itemCount > 0) {
                val text = clip.getItemAt(0).text?.toString() ?: ""
                if (text.isNotEmpty()) {
                    val syncIntent = Intent(this, KlipSyncService::class.java).apply {
                        action = KlipSyncService.ACTION_BROADCAST_CLIP
                        putExtra(KlipSyncService.EXTRA_CLIP_TEXT, text)
                    }
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                        startForegroundService(syncIntent)
                    } else {
                        startService(syncIntent)
                    }
                    val preview = if (text.length > 20) text.take(20) + "…" else text
                    Toast.makeText(this, "Synced to Klip: \"$preview\"", Toast.LENGTH_SHORT).show()
                } else {
                    Toast.makeText(this, "Klip: Clipboard is empty", Toast.LENGTH_SHORT).show()
                }
            } else {
                Toast.makeText(this, "Klip: Clipboard is empty", Toast.LENGTH_SHORT).show()
            }
        } catch (e: Exception) {
            Log.e(TAG, "Error in KlipTileService onClick: ${e.message}", e)
            Toast.makeText(this, "Klip sync error: ${e.message}", Toast.LENGTH_SHORT).show()
        }
    }
}
