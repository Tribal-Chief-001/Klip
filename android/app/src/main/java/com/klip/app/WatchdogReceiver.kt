package com.klip.app

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log

/**
 * Redundant heartbeat alarm receiver for the dual-watchdog architecture.
 *
 * JobScheduler periodic jobs are heavily deferred during Android Doze mode (maintenance windows
 * can be hours apart). This receiver handles exact/inexact while-idle alarms from AlarmManager,
 * ensuring KlipSyncService is promptly resurrected if terminated by an aggressive OEM cleaner.
 */
class WatchdogReceiver : BroadcastReceiver() {

    override fun onReceive(context: Context, intent: Intent) {
        if (!Watchdog.enabled(context)) {
            Log.d(Watchdog.TAG, "Watchdog receiver triggered but watchdog is disabled")
            return
        }

        if (!KlipSyncService.running) {
            Log.w(Watchdog.TAG, "KlipSyncService was not running; resurrecting via AlarmManager watchdog")
            KlipSyncService.start(context)
        } else {
            Log.d(Watchdog.TAG, "Watchdog heartbeat pulse: KlipSyncService is healthy")
        }

        // Also ensure framework notification listener binding is alive if enabled
        KlipNotificationListenerService.requestRebindIfEnabled(context)

        // Re-arm next watchdog alarm cycle
        Watchdog.scheduleAlarm(context, force = true)
    }
}
