package com.klip.app

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log

/**
 * Boot & system event receiver:
 * 1. Resumes KlipSyncService on boot and package update.
 * 2. Primes the dual-watchdog (JobScheduler + AlarmManager).
 * 3. Rebinds notification listener if enabled.
 */
class BootReceiver : BroadcastReceiver() {

    companion object {
        private const val TAG = "BootReceiver"
    }

    override fun onReceive(context: Context, intent: Intent) {
        val action = intent.action
        Log.i(TAG, "BootReceiver triggered with action: $action")
        when (action) {
            Intent.ACTION_BOOT_COMPLETED,
            Intent.ACTION_MY_PACKAGE_REPLACED -> {
                Watchdog.schedule(context)
                KlipSyncService.start(context)
                KlipNotificationListenerService.requestRebindIfEnabled(context)
            }
        }
    }
}
