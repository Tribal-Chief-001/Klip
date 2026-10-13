package com.klip.app

import android.app.PendingIntent
import android.content.Intent
import android.graphics.drawable.Icon
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.service.quicksettings.Tile
import android.service.quicksettings.TileService
import android.util.Log

/** Quick Settings tile: tap to send the current clipboard to the linked devices. */
class KlipTileService : TileService() {

    override fun onStartListening() {
        super.onStartListening()
        show(Tile.STATE_INACTIVE, "Send clipboard")
        // Lit when at least one linked device is connected; the subtitle says who. Engine calls block,
        // so ask off the main thread. Runs only while the panel is open.
        Thread {
            val s = runCatching { StatusText.compute() }.getOrNull()
            Handler(Looper.getMainLooper()).post {
                when {
                    s == null -> show(Tile.STATE_INACTIVE, "Send clipboard")
                    s.anyOnline -> show(Tile.STATE_ACTIVE, "Send to ${s.firstOnline}")
                    else -> show(Tile.STATE_INACTIVE, "Not connected")
                }
            }
        }.start()
    }

    private fun show(state: Int, subtitle: String) {
        qsTile?.let { tile ->
            tile.state = state
            tile.label = "Klip"
            tile.icon = Icon.createWithResource(this, R.drawable.ic_klip_logo)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) tile.subtitle = subtitle
            tile.updateTile()
        }
    }

    override fun onClick() {
        super.onClick()
        val launch = Runnable {
            try {
                val intent = Intent(this, CopyTrampolineActivity::class.java).apply {
                    action = CopyTrampolineActivity.ACTION_SEND_CLIPBOARD
                    flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP
                }
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
                    val pi = PendingIntent.getActivity(
                        this, 0, intent, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
                    )
                    startActivityAndCollapse(pi)
                } else {
                    @Suppress("DEPRECATION")
                    startActivityAndCollapse(intent)
                }
            } catch (e: Throwable) {
                Log.e("KlipTileService", "Tile action failed: ${e.message}", e)
            }
        }
        if (isLocked) unlockAndRun(launch) else launch.run()
    }
}
