package com.klip.app

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/**
 * Start the background service after a restart of the phone and after Klip itself was updated (an update
 * stops the app; without this it would stay off until opened), and make sure the watchdog job exists.
 */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action == Intent.ACTION_BOOT_COMPLETED || intent.action == Intent.ACTION_MY_PACKAGE_REPLACED) {
            Watchdog.schedule(context)
            KlipSyncService.start(context)
        }
    }
}
