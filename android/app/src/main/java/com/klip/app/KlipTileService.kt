package com.klip.app

import android.app.PendingIntent
import android.content.Intent
import android.graphics.drawable.Icon
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
            tile.label = "📎 Klip"
            tile.icon = Icon.createWithResource(this, R.drawable.ic_klip_logo)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                tile.subtitle = "Tap to sync clip"
            }
            tile.updateTile()
        }
    }

    override fun onClick() {
        super.onClick()
        Log.i(TAG, "Quick Settings Klip tile tapped")

        val launchAction = Runnable {
            try {
                val syncIntent = Intent(this, CopyTrampolineActivity::class.java).apply {
                    action = CopyTrampolineActivity.ACTION_TILE_SYNC
                    flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP
                }

                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
                    val pendingIntent = PendingIntent.getActivity(
                        this,
                        0,
                        syncIntent,
                        PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
                    )
                    startActivityAndCollapse(pendingIntent)
                } else {
                    @Suppress("DEPRECATION")
                    startActivityAndCollapse(syncIntent)
                }
            } catch (e: Exception) {
                Log.e(TAG, "Error in KlipTileService onClick: ${e.message}", e)
                Toast.makeText(this, "Klip sync error: ${e.message}", Toast.LENGTH_SHORT).show()
            }
        }

        if (isLocked) {
            unlockAndRun(launchAction)
        } else {
            launchAction.run()
        }
    }
}
