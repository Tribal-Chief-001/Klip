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
 * right screen.
 *
 * Special focus on Vivo / Funtouch OS 15:
 * Vivo has dual battery management:
 * 1. Standard AOSP App Info battery optimization (which exempts from Doze mode).
 * 2. Proprietary Wisdom Engine (com.vivo.abe / com.vivo.pem) with "Background power consumption management"
 *    where apps default to "Smart Control" and are force-stopped after 20-30 min of screen-off unless
 *    "Don't restrict background power usage" is explicitly chosen.
 */
class Oem(
    val id: String,
    val title: String,
    val intro: String,
    val steps: List<String>,
    /** False for stock-like Android where nothing extra is needed. */
    val needsExtraSteps: Boolean,
    private val autostartScreens: List<Pair<String, String>> = emptyList(),
    private val powerScreens: List<Pair<String, String>> = emptyList(),
    private val powerActions: List<String> = emptyList()
) {
    /** Try autostart screens; returns true if an activity was launched. */
    fun openAutostartSettings(a: Activity): Boolean {
        for ((pkg, cls) in autostartScreens) {
            try {
                val intent = Intent().setComponent(ComponentName(pkg, cls)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                intent.putExtra("package", a.packageName)
                intent.putExtra("packageName", a.packageName)
                intent.putExtra("packagename", a.packageName)
                intent.putExtra("pkg_name", a.packageName)
                intent.putExtra("extra_pkgname", a.packageName)
                a.startActivity(intent)
                return true
            } catch (_: Throwable) { /* try next */ }
        }
        return false
    }

    /** Try maker's proprietary background power consumption screens; returns true if launched. */
    fun openPowerManagementSettings(a: Activity): Boolean {
        for ((pkg, cls) in powerScreens) {
            try {
                val intent = Intent().setComponent(ComponentName(pkg, cls)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                intent.putExtra("package", a.packageName)
                intent.putExtra("packageName", a.packageName)
                intent.putExtra("packagename", a.packageName)
                intent.putExtra("pkg_name", a.packageName)
                intent.putExtra("extra_pkgname", a.packageName)
                a.startActivity(intent)
                return true
            } catch (_: Throwable) { /* try next */ }
        }
        for (action in powerActions) {
            try {
                val intent = Intent(action).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                intent.putExtra("package", a.packageName)
                intent.putExtra("packageName", a.packageName)
                intent.putExtra("packagename", a.packageName)
                intent.putExtra("pkg_name", a.packageName)
                intent.putExtra("extra_pkgname", a.packageName)
                a.startActivity(intent)
                return true
            } catch (_: Throwable) { /* try next */ }
        }
        return false
    }

    /** Try the maker's proprietary power and autostart screens; fall back to Klip's app-info page. */
    fun openSettings(a: Activity) {
        if (openPowerManagementSettings(a)) return
        if (openAutostartSettings(a)) return
        openAppInfo(a)
    }

    companion object {
        fun openAppInfo(a: Activity) {
            try {
                a.startActivity(
                    Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:${a.packageName}"))
                )
            } catch (_: Throwable) {
                try {
                    a.startActivity(Intent(Settings.ACTION_SETTINGS))
                } catch (_: Throwable) {}
            }
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

        val VIVO = Oem(
            "vivo", "Vivo / iQOO",
            "Vivo's Funtouch OS aggressively terminates background services unless both Autostart and Background Power are configured. Complete these steps:",
            listOf(
                "Autostart: Settings → Applications / Battery → Autostart → turn Klip ON.",
                "Background power: Settings → Battery → Background power consumption management (or “High background power consumption”) → Klip → choose “Don't restrict background power usage” (Smart Control kills Klip after 30 min).",
                "App info battery: Long-press Klip icon → App info → Battery → set to Unrestricted.",
                "Recent apps lock: Open Recent apps (swipe up and hold) → pull Klip card down or tap ⋮ → Lock (padlock icon)."
            ),
            true,
            autostartScreens = listOf(
                "com.vivo.permissionmanager" to "com.vivo.permissionmanager.activity.BgStartUpManagerActivity",
                "com.vivo.permissionmanager" to "com.vivo.permissionmanager.activity.PurviewTabActivity",
                "com.vivo.permissionmanager" to "com.vivo.permissionmanager.activity.SoftPermissionDetailActivity",
                "com.iqoo.secure" to "com.iqoo.secure.ui.phoneoptimize.AddWhiteListActivity",
                "com.iqoo.secure" to "com.iqoo.secure.MainGuideActivity"
            ),
            powerScreens = listOf(
                "com.vivo.abe" to "com.vivo.applicationbehaviorengine.ui.ExcessivePowerManagerActivity",
                "com.vivo.abe" to "com.vivo.applicationbehaviorengine.ui.BatteryManagementActivity",
                "com.vivo.pem" to "com.vivo.pem.activity.ExcessivePowerManagerActivity",
                "com.vivo.pem" to "com.vivo.pem.activity.BatteryManagementActivity",
                "com.iqoo.secure" to "com.iqoo.secure.ui.phoneoptimize.AddWhiteListActivity"
            ),
            powerActions = listOf(
                "com.vivo.abe.uni.action.EXTERNAL_ACCESS"
            )
        )

        val XIAOMI = Oem(
            "xiaomi", "Xiaomi / Redmi / Poco",
            "MIUI and HyperOS stop background apps aggressively. Complete these steps:",
            listOf(
                "Autostart: Settings → Apps → Klip → Autostart: turn ON (or Security → Permissions → Autostart).",
                "Battery saver: Settings → Apps → Klip → Battery saver → choose No restrictions.",
                "Recent apps lock: Open Recent apps and lock Klip (pull the card down or long-press and tap the padlock)."
            ),
            true,
            autostartScreens = listOf(
                "com.miui.securitycenter" to "com.miui.permcenter.autostart.AutoStartManagementActivity"
            ),
            powerScreens = listOf(
                "com.miui.powerkeeper" to "com.miui.powerkeeper.ui.HiddenAppsConfigActivity"
            )
        )

        val OPPO = Oem(
            "oppo", "Oppo / Realme / OnePlus",
            "ColorOS-based phones stop background apps. Complete these steps:",
            listOf(
                "Auto-launch: Settings → Apps → App management → Klip → turn ON Auto-launch / Allow auto start-up.",
                "Battery usage: In the same screen → Battery usage → allow Run in background (or Unrestricted).",
                "Recent apps lock: Open Recent apps and lock Klip (tap ⋮ on the card → Lock)."
            ),
            true,
            autostartScreens = listOf(
                "com.coloros.safecenter" to "com.coloros.safecenter.permission.startup.StartupAppListActivity",
                "com.oppo.safe" to "com.oppo.safe.permission.startup.StartupAppListActivity"
            ),
            powerScreens = listOf(
                "com.coloros.oppoguardelf" to "com.coloros.powermanager.fuelgaue.PowerUsageModelActivity"
            )
        )

        val SAMSUNG = Oem(
            "samsung", "Samsung",
            "One UI puts unused apps to sleep. Complete these steps:",
            listOf(
                "Background usage: Settings → Battery → Background usage limits → add Klip to “Never sleeping apps”.",
                "Unrestricted: Settings → Apps → Klip → Battery → choose Unrestricted.",
                "Adaptive battery: Settings → Battery → More battery settings → turn off “Adaptive battery” if issues persist."
            ),
            true,
            powerScreens = listOf(
                "com.samsung.android.lool" to "com.samsung.android.sm.ui.battery.BatteryActivity",
                "com.samsung.android.sm" to "com.samsung.android.sm.ui.battery.BatteryActivity"
            )
        )

        val HUAWEI = Oem(
            "huawei", "Huawei / Honor",
            "EMUI/MagicOS manage app launch strictly. Complete these steps:",
            listOf(
                "App launch: Settings → Apps → App launch → Klip → switch to Manage manually and allow Auto-launch, Secondary launch and Run in background.",
                "Battery optimisation: Settings → Battery → turn off battery optimisation for Klip.",
                "Recent apps lock: Open Recent apps and lock Klip."
            ),
            true,
            autostartScreens = listOf(
                "com.huawei.systemmanager" to "com.huawei.systemmanager.startupmgr.ui.StartupNormalAppListActivity"
            ),
            powerScreens = listOf(
                "com.huawei.systemmanager" to "com.huawei.systemmanager.power.ui.HwPowerManagerActivity"
            )
        )

        val STOCK = Oem(
            "stock", "your phone",
            "Your phone uses standard Android power management. Battery → Unrestricted is sufficient.",
            listOf("If Klip stops in the background: long-press the Klip icon → App info → Battery → Unrestricted."),
            false
        )
    }
}
