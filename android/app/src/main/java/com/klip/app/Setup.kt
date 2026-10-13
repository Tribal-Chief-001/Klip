package com.klip.app

import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.os.PowerManager

/** What has to be true for Klip to keep running in the background, and what we can check. */
object Setup {
    enum class State { DONE, TODO, UNCHECKABLE_TODO }

    class Item(val id: String, val state: State, val required: Boolean)

    private const val PREFS = "klip_ui"

    fun notificationsGranted(c: Context) =
        Build.VERSION.SDK_INT < 33 ||
            c.checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED

    fun batteryUnrestricted(c: Context) =
        (c.getSystemService(Context.POWER_SERVICE) as PowerManager).isIgnoringBatteryOptimizations(c.packageName)

    /**
     * Android cannot tell an app everything: phone makers (Vivo, Xiaomi, ...) keep their own switches that
     * never show up in the standard checks. So every item can also be marked done by the person.
     */
    fun confirmed(c: Context, id: String): Boolean {
        val p = c.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        return p.getBoolean("confirmed_$id", false) || (id == "autostart" && p.getBoolean("autostart_confirmed", false))
    }

    fun setConfirmed(c: Context, id: String, v: Boolean) {
        c.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
            .putBoolean("confirmed_$id", v).remove("autostart_confirmed").apply()
    }

    /** The required items, in the order the setup screen shows them. */
    fun required(c: Context): List<Item> {
        fun item(id: String, auto: Boolean?, manualOnly: Boolean = false) = Item(
            id,
            when {
                auto == true || confirmed(c, id) -> State.DONE
                manualOnly -> State.UNCHECKABLE_TODO
                else -> State.TODO
            },
            true
        )
        val list = mutableListOf<Item>()
        if (Build.VERSION.SDK_INT >= 33) list += item("notifications", notificationsGranted(c))
        list += item("battery", batteryUnrestricted(c))
        if (Oem.detect().needsExtraSteps) list += item("autostart", null, manualOnly = true)
        return list
    }

    fun openCount(c: Context) = required(c).count { it.state != State.DONE }
}
