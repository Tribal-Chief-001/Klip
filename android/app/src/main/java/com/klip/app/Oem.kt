package com.klip.app

import android.app.Activity
import android.content.ComponentName
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.Settings

/**
 * Phone makers add their own "kill background apps" layers on top of Android. Apps cannot read or
 * change those settings, so Klip can only (a) say exactly where they are and (b) try to open the
 * right screen. The step texts come from the community list dontkillmyapp.com (read 2026-10-13);
 * menu names change between versions, so they are guidance, not a promise. The deep links below
 * are community-known component names that were NOT verified on real devices: each is tried in
 * order and the app's own info page is the fallback.
 */
class Oem(
    val id: String,
    val title: String,
    val intro: String,
    val steps: List<String>,
    /** False for stock-like Android where nothing extra is needed. */
    val needsExtraSteps: Boolean,
    private val screens: List<Pair<String, String>> = emptyList()
) {
    /** Try the maker's auto-start/battery screens; fall back to Klip's app-info page. */
    fun openSettings(a: Activity) {
        for ((pkg, cls) in screens) {
            try {
                a.startActivity(Intent().setComponent(ComponentName(pkg, cls)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                return
            } catch (_: Throwable) { /* not on this phone: try the next */ }
        }
        openAppInfo(a)
    }

    companion object {
        fun openAppInfo(a: Activity) {
            a.startActivity(
                Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:${a.packageName}"))
            )
        }

        fun detect(): Oem {
            val maker = Build.MANUFACTURER.lowercase()
            val brand = Build.BRAND.lowercase()
            fun has(vararg k: String) = k.any { maker.contains(it) || brand.contains(it) }
            return when {
                has("vivo", "iqoo") -> VIVO
                has("xiaomi", "redmi", "poco") -> XIAOMI
                has("oppo", "realme", "oneplus") -> OPPO
                has("samsung") -> SAMSUNG
                has("huawei", "honor") -> HUAWEI
                else -> STOCK
            }
        }

        private val VIVO = Oem(
            "vivo", "Vivo / iQOO",
            "Vivo's battery manager stops background apps. Do all of these:",
            listOf(
                "Settings → More settings → Applications → Autostart: turn Klip ON.",
                "Settings → Battery → Background power consumption management (or “High background power consumption”): find Klip and allow it.",
                "Long-press the Klip icon → App info → Battery → set to Unrestricted / Not optimized.",
                "Open Recent apps, find Klip, and lock it (pull the card down or use its menu → Lock)."
            ),
            true,
            listOf(
                "com.vivo.permissionmanager" to "com.vivo.permissionmanager.activity.BgStartUpManagerActivity",
                "com.vivo.permissionmanager" to "com.vivo.permissionmanager.activity.PurviewTabActivity",
                "com.iqoo.secure" to "com.iqoo.secure.ui.phoneoptimize.AddWhiteListActivity"
            )
        )

        private val XIAOMI = Oem(
            "xiaomi", "Xiaomi / Redmi / Poco",
            "MIUI and HyperOS stop background apps aggressively. Do all of these:",
            listOf(
                "Settings → Apps → Klip → Autostart: turn ON (older versions: Security → Permissions → Autostart).",
                "Settings → Apps → Klip → Battery saver: choose No restrictions.",
                "Open Recent apps and lock Klip (pull the card down, or long-press it and tap the padlock)."
            ),
            true,
            listOf("com.miui.securitycenter" to "com.miui.permcenter.autostart.AutoStartManagementActivity")
        )

        private val OPPO = Oem(
            "oppo", "Oppo / Realme / OnePlus",
            "ColorOS-based phones stop background apps. Names differ a little by brand and version:",
            listOf(
                "Settings → Apps → App management → Klip → turn ON Auto-launch / Allow auto start-up.",
                "In the same screen: Battery usage → allow Run in background (or Unrestricted).",
                "Open Recent apps and lock Klip.",
                "Check these again after a big system update; they can be reset."
            ),
            true,
            listOf(
                "com.coloros.safecenter" to "com.coloros.safecenter.permission.startup.StartupAppListActivity",
                "com.oppo.safe" to "com.oppo.safe.permission.startup.StartupAppListActivity"
            )
        )

        private val SAMSUNG = Oem(
            "samsung", "Samsung",
            "One UI puts unused apps to sleep. Do these:",
            listOf(
                "Settings → Battery → Background usage limits: make sure Klip is not in “Sleeping apps” or “Deep sleeping apps”; add it to “Never sleeping apps”.",
                "Settings → Apps → Klip → Battery → choose Unrestricted.",
                "Settings → Battery → More battery settings: turn off “Put unused apps to sleep” and “Adaptive battery”."
            ),
            true,
            listOf("com.samsung.android.lool" to "com.samsung.android.sm.ui.battery.BatteryActivity")
        )

        private val HUAWEI = Oem(
            "huawei", "Huawei / Honor",
            "EMUI/MagicOS manage app launch by themselves. Do these:",
            listOf(
                "Settings → Apps → App launch → Klip → switch to Manage manually and allow Auto-launch, Secondary launch and Run in background.",
                "Settings → Battery → turn off battery optimisation for Klip.",
                "Open Recent apps and lock Klip."
            ),
            true,
            listOf("com.huawei.systemmanager" to "com.huawei.systemmanager.startupmgr.ui.StartupNormalAppListActivity")
        )

        private val STOCK = Oem(
            "stock", "your phone",
            "Your phone has no extra auto-start screen. Battery → Unrestricted (above) is enough.",
            listOf("If Klip ever stops in the background: long-press the Klip icon → App info → Battery → Unrestricted."),
            false
        )
    }
}
