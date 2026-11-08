package com.klip.app

import android.app.ActivityManager
import android.content.Context
import android.content.Intent
import android.util.Log

/**
 * Manages anchoring Klip into Android's Recent Apps carousel.
 *
 * Why this is vital on Vivo / Funtouch OS 15:
 * On boot, a headless background service has zero cards in Recent Apps. Funtouch OS's background
 * sweep terminates any process without a Recent Apps task record and makes it impossible for
 * the user to padlock the app.
 *
 * Anchoring launches a lightweight activity record. When anchorOnly is true, MainActivity
 * calls moveTaskToBack(true) to establish the Recent Apps card without interrupting the user.
 */
object TaskAnchor {
    private const val TAG = "TaskAnchor"

    /** Returns true if Klip currently has an active task card in Android's Recent Apps carousel. */
    fun hasRecentTask(context: Context): Boolean {
        val am = context.getSystemService(Context.ACTIVITY_SERVICE) as? ActivityManager ?: return false
        return runCatching {
            am.appTasks.isNotEmpty()
        }.getOrDefault(false)
    }

    /**
     * Tries to launch MainActivity to establish a card in Recent Apps.
     * On OEM ROMs where Autostart is allowed (like Vivo), background activity launch is permitted.
     * If blocked by Android 10+ BAL restrictions, it safely catches the error and returns false.
     */
    fun tryBootstrapAnchor(context: Context, anchorOnly: Boolean = true): Boolean {
        if (hasRecentTask(context)) {
            Log.d(TAG, "Recent Apps card already exists; skipping bootstrap")
            return true
        }

        return try {
            val intent = Intent(context, MainActivity::class.java).apply {
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                putExtra(MainActivity.EXTRA_FROM_BOOT, true)
                if (anchorOnly) {
                    putExtra(MainActivity.EXTRA_ANCHOR_ONLY, true)
                }
            }
            context.startActivity(intent)
            Log.i(TAG, "Task anchor activity successfully launched (anchorOnly=$anchorOnly)")
            true
        } catch (e: Throwable) {
            Log.w(TAG, "Cannot launch task anchor activity from background: ${e.message}")
            false
        }
    }
}
