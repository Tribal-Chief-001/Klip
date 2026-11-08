package com.klip.app

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.os.Build
import android.provider.Settings
import android.service.notification.NotificationListenerService
import android.service.notification.StatusBarNotification
import android.util.Log

/**
 * System-bound notification listener that anchors Klip to Android's system_server.
 *
 * Why this exists:
 * Aggressive OEM power managers (such as Vivo's com.vivo.abe / Wisdom Engine) periodically sweep
 * background processes and invoke forceStopPackage. However, system_server maintains a persistent
 * IPC binding to any enabled NotificationListenerService. OEM task cleaners deliberately exempt
 * apps with active listener bindings to avoid breaking smartwatches and wearable companions.
 * If the process is ever killed by memory pressure, system_server automatically re-spawns the
 * process and re-binds.
 *
 * Privacy Invariant:
 * Klip uses this service STRICTLY as an architectural keep-alive anchor. It does NOT read,
 * log, store, or transmit any notifications from other applications.
 */
class KlipNotificationListenerService : NotificationListenerService() {

    companion object {
        private const val TAG = "KlipNotificationListener"
        @Volatile var isBound = false
            private set

        /** Returns true if the user has enabled Klip in Notification Access settings. */
        fun isEnabled(context: Context): Boolean {
            val enabledListeners = Settings.Secure.getString(
                context.contentResolver,
                "enabled_notification_listeners"
            ) ?: return false
            val myComponent = ComponentName(context, KlipNotificationListenerService::class.java).flattenToString()
            val myShortComponent = ComponentName(context, KlipNotificationListenerService::class.java).flattenToShortString()
            return enabledListeners.split(":").any {
                it.equals(myComponent, ignoreCase = true) || it.equals(myShortComponent, ignoreCase = true)
            }
        }

        /** Opens Android's Notification Access settings screen. */
        fun openSettings(context: Context) {
            val intent = Intent(Settings.ACTION_NOTIFICATION_LISTENER_SETTINGS).apply {
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                    putExtra(
                        Settings.EXTRA_NOTIFICATION_LISTENER_COMPONENT_NAME,
                        ComponentName(context, KlipNotificationListenerService::class.java).flattenToString()
                    )
                }
            }
            try {
                context.startActivity(intent)
            } catch (e: Throwable) {
                Log.w(TAG, "Cannot open notification listener settings: ${e.message}")
            }
        }

        /** Explicitly requests system_server to re-bind this service if access has been granted. */
        fun requestRebindIfEnabled(context: Context) {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N && isEnabled(context) && !isBound) {
                try {
                    requestRebind(ComponentName(context, KlipNotificationListenerService::class.java))
                    Log.d(TAG, "Requested system_server rebind for NotificationListenerService")
                } catch (e: Throwable) {
                    Log.w(TAG, "Failed to request rebind: ${e.message}")
                }
            }
        }
    }

    override fun onCreate() {
        super.onCreate()
        isBound = true
        Log.i(TAG, "KlipNotificationListenerService created; ensuring KlipSyncService is running")
        KlipSyncService.start(this)
    }

    override fun onListenerConnected() {
        super.onListenerConnected()
        isBound = true
        Log.i(TAG, "Notification listener connected to system_server")
        KlipSyncService.start(this)
    }

    override fun onListenerDisconnected() {
        super.onListenerDisconnected()
        isBound = false
        Log.w(TAG, "Notification listener disconnected from system_server; requesting rebind")
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            runCatching {
                requestRebind(ComponentName(this, KlipNotificationListenerService::class.java))
            }.onFailure { e ->
                Log.w(TAG, "Rebind request failed: ${e.message}")
            }
        }
    }

    override fun onDestroy() {
        isBound = false
        super.onDestroy()
    }

    override fun onNotificationPosted(sbn: StatusBarNotification?) {
        // Explicitly NO-OP: Klip does not inspect notifications from other apps.
    }

    override fun onNotificationRemoved(sbn: StatusBarNotification?) {
        // Explicitly NO-OP
    }
}
