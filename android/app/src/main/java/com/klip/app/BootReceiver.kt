package com.klip.app

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/** Restart the background service after a reboot so devices stay linked without opening the app. */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action == Intent.ACTION_BOOT_COMPLETED) {
            KlipSyncService.start(context)
        }
    }
}
