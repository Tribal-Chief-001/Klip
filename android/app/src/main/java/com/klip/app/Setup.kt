package com.klip.app

import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.os.PowerManager
import android.provider.Settings

/** What has to be true for Klip to keep running in the background, and what we can check. */
object Setup {
    enum class State { DONE, TODO, UNCHECKABLE_TODO }

    class Item(val id: String, val state: State, val required: Boolean)

    private const val PREFS = "klip_ui"
    private const val KEY_AUTOSTART = "autostart_confirmed"

    fun notificationsGranted(c: Context) =
        Build.VERSION.SDK_INT < 33 ||
            c.checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED

    fun batteryUnrestricted(c: Context) =
        (c.getSystemService(Context.POWER_SERVICE) as PowerManager).isIgnoringBatteryOptimizations(c.packageName)

    fun autostartConfirmed(c: Context) =
        c.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean(KEY_AUTOSTART, false)

    fun setAutostartConfirmed(c: Context, v: Boolean) =
        c.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putBoolean(KEY_AUTOSTART, v).apply()

    fun logsGranted(c: Context) =
        c.checkSelfPermission(android.Manifest.permission.READ_LOGS) == PackageManager.PERMISSION_GRANTED

    fun overlayAllowed(c: Context) = Settings.canDrawOverlays(c)

    /** The required items, in the order the setup screen shows them. */
    fun required(c: Context): List<Item> {
        val list = mutableListOf<Item>()
        if (Build.VERSION.SDK_INT >= 33) {
            list += Item("notifications", if (notificationsGranted(c)) State.DONE else State.TODO, true)
        }
        list += Item("battery", if (batteryUnrestricted(c)) State.DONE else State.TODO, true)
        val oem = Oem.detect()
        if (oem.needsExtraSteps) {
            list += Item("autostart", if (autostartConfirmed(c)) State.DONE else State.UNCHECKABLE_TODO, true)
        }
        return list
    }

    fun openCount(c: Context) = required(c).count { it.state != State.DONE }
}
