package com.klip.app

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log

/**
 * Boot & system event receiver:
 * 1. Resumes KlipSyncService on boot, package update, and user unlock.
 * 2. Primes the dual-watchdog (JobScheduler + AlarmManager).
 * 3. Bootstraps a task anchor into Recent Apps so users can padlock Klip across reboots.
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
                // Establish Recent Apps task card so the user can lock/padlock it
                TaskAnchor.tryBootstrapAnchor(context, anchorOnly = true)
            }
            Intent.ACTION_USER_PRESENT -> {
                // Fired when the user unlocks the screen
                Watchdog.schedule(context)
                if (!KlipSyncService.running) {
                    Log.i(TAG, "User present and KlipSyncService not running; starting service")
                    KlipSyncService.start(context)
                }
                if (!TaskAnchor.hasRecentTask(context)) {
                    TaskAnchor.tryBootstrapAnchor(context, anchorOnly = true)
                }
            }
        }
    }
}
