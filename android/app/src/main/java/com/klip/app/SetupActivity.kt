package com.klip.app

import android.app.Activity
import android.app.StatusBarManager
import android.content.ClipData
import android.content.ClipboardManager
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.RectF
import android.graphics.drawable.Icon
import android.net.Uri
import android.os.Build
import android.provider.Settings
import android.view.Gravity
import android.view.View
import android.widget.LinearLayout
import android.widget.TextView
import android.widget.Toast
import com.klip.app.ui.Page
import com.klip.app.ui.Pal
import com.klip.app.ui.add
import com.klip.app.ui.card
import com.klip.app.ui.chip
import com.klip.app.ui.column
import com.klip.app.ui.dp
import com.klip.app.ui.dpf
import com.klip.app.ui.primaryButton
import com.klip.app.ui.secondaryButton
import com.klip.app.ui.sectionLabel
import com.klip.app.ui.textButton
import com.klip.app.ui.tv

/**
 * "Keep Klip running": every permission and phone setting Klip needs, one card each, with what it does,
 * what it can and cannot see, and the exact steps. Items Android lets us check show Done by themselves;
 * the maker-specific ones cannot be read by any app, so the person confirms them.
 */
class SetupActivity : Activity() {

    private lateinit var pal: Pal
    private lateinit var page: Page
    private val expanded = mutableSetOf<String>()
    private var seeded = false
    private var notificationAsked = false

    override fun onCreate(savedInstanceState: android.os.Bundle?) {
        super.onCreate(savedInstanceState)
        pal = Pal.of(this)
        page = Page(this, pal)
    }

    override fun onResume() {
        super.onResume()
        render()
        window.decorView.postDelayed(statusTicker, 2_000)
    }

    @Deprecated("Deprecated in Java")
    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        @Suppress("DEPRECATION")
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        render()
    }

    // --------------------------------------------------------------------------------------- UI

    private fun render() {
        val b = page.body
        b.removeAllViews()

        val top = LinearLayout(this).apply { gravity = Gravity.CENTER_VERTICAL }
        top.addView(textButton(this, pal, "‹ Back", pal.text) { finish() })
        b.add(top, bottom = 8)

        b.add(tv("Keep Klip running", 28f, pal.text, 2), bottom = 8)
        b.add(
            tv("Phones stop background apps to save battery, and some makers do it harder than Android does. " +
                "Do these once and Klip stays connected with the screen off and after a restart. " +
                "None of them gives Klip access to your messages, photos, contacts or accounts.", 14.5f, pal.dim),
            bottom = 18
        )

        val req = Setup.required(this)
        val done = req.count { it.state == Setup.State.DONE }
        b.add(Segments(this, pal, req.size, done), bottom = 8)
        b.add(
            tv(if (done == req.size) "All set. Klip can run in the background." else "$done of ${req.size} done", 13.5f,
                if (done == req.size) pal.signal else pal.text, 1),
            bottom = 18
        )
        if (!seeded) {
            seeded = true
            req.filter { it.state != Setup.State.DONE }.forEach { expanded += it.id }
        }

        if (Build.VERSION.SDK_INT >= 33) b.add(notificationsCard(), bottom = 12)
        b.add(batteryCard(), bottom = 12)
        b.add(autostartCard(), bottom = 26)

        b.add(sectionLabel(this, pal, "Optional"), bottom = 10)
        b.add(tileCard(), bottom = 12)
        b.add(shareCard(), bottom = 12)
        b.add(autoCopyCard(), bottom = 12)
        b.add(testCard(), bottom = 12)
    }

    /** A collapsible card: header (title, chip, one line) and a body shown when expanded. */
    private fun item(
        id: String, title: String, summary: String, state: Setup.State?, optionalLabel: String? = null,
        body: LinearLayout.() -> Unit
    ): View {
        val c = card(this, pal)
        val head = LinearLayout(this).apply { gravity = Gravity.CENTER_VERTICAL }
        val texts = column(this)
        texts.add(tv(title, 16.5f, pal.text, 1))
        texts.add(tv(summary, 13.5f, pal.dim), top = 3)
        head.addView(texts, LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f))
        val tag = when {
            state == Setup.State.DONE -> chip(this, "Done", pal.signal)
            state != null -> chip(this, "To do", pal.warn)
            optionalLabel != null -> chip(this, optionalLabel, pal.dim)
            else -> null
        }
        if (tag != null) head.addView(tag, LinearLayout.LayoutParams(-2, -2).apply { leftMargin = dp(10) })
        c.addView(head)

        val details = column(this).apply {
            visibility = if (id in expanded) View.VISIBLE else View.GONE
            body()
        }
        c.addView(details)
        head.isClickable = true
        head.setOnClickListener {
            val open = details.visibility != View.VISIBLE
            details.visibility = if (open) View.VISIBLE else View.GONE
            if (open) expanded += id else expanded -= id
        }
        return c
    }

    private fun LinearLayout.block(label: String, text: String) {
        add(sectionLabel(this@SetupActivity, pal, label), top = 16, bottom = 6)
        add(tv(text, 14f, pal.text))
    }

    private fun LinearLayout.steps(list: List<String>) {
        list.forEachIndexed { i, s ->
            val row = LinearLayout(this@SetupActivity)
            row.addView(tv("${i + 1}", 13f, pal.faint, 1, mono = true), LinearLayout.LayoutParams(dp(22), -2))
            row.addView(tv(s, 14f, pal.text), LinearLayout.LayoutParams(0, -2, 1f))
            add(row, top = if (i == 0) 0 else 8)
        }
    }

    // ----------------------------------------------------------------------------- required items

    private fun notificationsCard(): View {
        val ok = Setup.notificationsGranted(this)
        return item("notifications", "Notifications", "Shows that Klip is running.",
            stateOf("notifications", ok)) {
            block("What it does",
                "Android makes any app that runs all the time show a small notification. Klip uses it for status and a “Send clipboard” button.")
            block("What it can and can't see", "Klip can only show its own notification. It cannot read yours.")
            manualControls("notifications", ok)
            if (!ok) {
                add(primaryButton(this@SetupActivity, pal,
                    if (notificationAsked) "Open notification settings" else "Allow notifications") {
                    if (notificationAsked) {
                        startActivity(Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS)
                            .putExtra(Settings.EXTRA_APP_PACKAGE, packageName))
                    } else {
                        notificationAsked = true
                        requestPermissions(arrayOf(android.Manifest.permission.POST_NOTIFICATIONS), 11)
                    }
                }, top = 16)
            }
        }
    }

    private fun batteryCard(): View {
        val ok = Setup.batteryUnrestricted(this)
        return item("battery", "Unrestricted battery use", "Stops Android from pausing Klip when the screen is off.",
            stateOf("battery", ok)) {
            block("What it does",
                "Android normally freezes background apps after a while. Unrestricted tells it not to freeze Klip, so your devices stay connected.")
            block("What it can and can't see",
                "It gives Klip no extra data access. It does cost a little battery: Klip keeps one network connection open. " +
                    "I have not measured how much on real phones yet.")
            val oem = Oem.detect()
            if (oem.needsExtraSteps) {
                add(tv("On ${oem.title} phones this Android switch is separate from the maker's own battery manager " +
                    "(see the next card), and Android can't see what you set there. If you already allowed Klip there, " +
                    "mark this as done below.", 13f, pal.warn), top = 12)
            }
            manualControls("battery", ok)
            if (!ok) {
                add(primaryButton(this@SetupActivity, pal, "Allow") {
                    try {
                        startActivity(Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:$packageName")))
                    } catch (_: Throwable) {
                        startActivity(Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS))
                    }
                }, top = 16)
            }
        }
    }

    private fun autostartCard(): View {
        val oem = Oem.detect()
        if (!oem.needsExtraSteps) {
            return item("autostart", "Auto-start & background", "Nothing extra needed on ${oem.title}.", Setup.State.DONE) {
                block("What to know", oem.intro + "\n" + oem.steps.first())
            }
        }
        val confirmed = Setup.confirmed(this, "autostart")
        return item("autostart", "Auto-start & background (${oem.title})",
            "Your phone's own battery manager. Android apps can't check this one.",
            stateOf("autostart", false)) {
            block("What it does", oem.intro + " Without these, Klip is closed a few minutes after the screen turns off and does not come back after a restart.")
            block("What it can and can't see", "These are your phone maker's switches. They only decide whether Klip may run; they give it no extra data access.")
            add(sectionLabel(this@SetupActivity, pal, "Steps"), top = 16, bottom = 8)
            steps(oem.steps)
            add(tv("Menu names differ between versions. If you can't find one, search for it in Settings.", 12.5f, pal.dim), top = 10)
            add(secondaryButton(this@SetupActivity, pal, "Open the settings screen") { oem.openSettings(this@SetupActivity) }, top = 14)
            if (!confirmed) {
                add(tv("Android doesn't let apps see these switches, so tell Klip when you've done them.", 12.5f, pal.dim), top = 10)
            }
            manualControls("autostart", false, primary = true)
        }
    }

    // ------------------------------------------------------------------------ manual confirmation

    private fun stateOf(id: String, auto: Boolean) =
        if (auto || Setup.confirmed(this, id)) Setup.State.DONE else Setup.State.TODO

    /**
     * Android can't see every setting (especially phone makers' own), so each required item can be ticked
     * off by hand. Shows nothing when the system already confirms it.
     */
    private fun LinearLayout.manualControls(id: String, autoOk: Boolean, primary: Boolean = false) {
        if (autoOk) return
        if (!Setup.confirmed(this@SetupActivity, id)) {
            val label = if (primary) "I've done these steps" else "I already did this: mark as done"
            val act = { Setup.setConfirmed(this@SetupActivity, id, true); render() }
            if (primary) add(primaryButton(this@SetupActivity, pal, label, act), top = 8)
            else add(textButton(this@SetupActivity, pal, label) { act() }, top = 8)
        } else {
            add(tv("Marked as done by you.", 13f, pal.signal, 1), top = 12)
            add(textButton(this@SetupActivity, pal, "Undo", pal.dim) {
                Setup.setConfirmed(this@SetupActivity, id, false); render()
            }, top = 0)
        }
    }

    // ----------------------------------------------------------------------------- optional items

    private fun tileCard(): View = item("tile", "Quick Settings tile", "One tap to send what you copied.", null, "Optional") {
        block("What it does", "Adds a Klip button to the panel you pull down from the top. Copy something, pull down, tap Klip.")
        block("What it can and can't see", "The tile only reads the clipboard at the moment you tap it.")
        if (Build.VERSION.SDK_INT >= 33) {
            add(secondaryButton(this@SetupActivity, pal, "Add the tile") { requestTile() }, top = 14)
        } else {
            add(tv("Swipe down twice, tap the pencil (edit), and drag Klip into the panel.", 14f, pal.text), top = 12)
        }
    }

    private fun requestTile() {
        if (Build.VERSION.SDK_INT < 33) return
        val sbm = getSystemService(StatusBarManager::class.java)
        sbm.requestAddTileService(
            ComponentName(this, KlipTileService::class.java), "Klip",
            Icon.createWithResource(this, R.drawable.ic_klip_logo), mainExecutor
        ) { result ->
            val msg = when (result) {
                StatusBarManager.TILE_ADD_REQUEST_RESULT_TILE_ADDED -> "Tile added."
                StatusBarManager.TILE_ADD_REQUEST_RESULT_TILE_ALREADY_ADDED -> "The tile is already there."
                else -> "Not added. You can add it by hand: swipe down twice, tap the pencil."
            }
            Toast.makeText(this, msg, Toast.LENGTH_SHORT).show()
        }
    }

    private fun shareCard(): View = item("share", "Send from any app", "Share menu and text selection.", null, "Info") {
        block("How",
            "Select text in any app, then Share → Klip. Or select text and look for “Send to Klip” in the pop-up menu (it may be under ⋮).")
        block("What it can and can't see", "Klip receives only the text you chose to share.")
    }

    private var helperStatus: TextView? = null
    private val statusTicker = object : Runnable {
        override fun run() {
            helperStatus?.apply {
                text = helperStatusText()
                setTextColor(if (ClipHelperServer.connected) pal.signal else pal.dim)
            }
            window.decorView.postDelayed(this, 2_000)
        }
    }

    private fun helperStatusText(): String = when {
        ClipHelperServer.connected -> "Helper running: copies are sent automatically."
        AutoCopy.rootEnabled(this) -> (AutoCopy.lastRootResult ?: "Waiting for root to start the helper…")
        else -> "Helper not running."
    }

    override fun onPause() {
        window.decorView.removeCallbacks(statusTicker)
        super.onPause()
    }

    private fun autoCopyCard(): View {
        val running = ClipHelperServer.connected
        return item("auto", "Automatic copy from this phone",
            if (running) "Running." else "Optional. Needs a computer once, or a rooted phone.", null,
            if (running) "Running" else "Advanced") {
            block("The problem",
                "Since Android 10 only the app on screen may read the clipboard, so a copy made in another app can't be noticed by Klip. " +
                    "The tile, Share and the notification button work without any setup. This is how to get fully automatic sending.")
            block("The helper",
                "A small helper process that runs with the same level of access as Android's adb shell, which is allowed to read the clipboard " +
                    "in the background (scrcpy works the same way). It watches the clipboard and hands each copy to Klip. " +
                    "No log reading, no “display over other apps”, no accessibility service. " +
                    "(The older log-reading trick no longer works on Android 13 and newer.)")
            block("What it costs",
                "It is a separate Java process: about 60 MB of memory while it runs (measured on an emulator), on top of Klip. " +
                    "Without root it stops when the phone restarts and has to be started again.")
            block("What it can see",
                "Everything you copy, while it runs, except what an app marks as sensitive (password managers do). It passes that only to Klip on " +
                    "this phone, over a local connection that both sides prove a secret token for; it stores nothing.")

            add(sectionLabel(this@SetupActivity, pal, "From a computer"), top = 16, bottom = 8)
            steps(listOf(
                "Settings → About phone → tap Build number 7 times, then Developer options → USB debugging ON. Connect the phone to a computer with adb.",
                "On the computer, run the command below (tap Copy). It starts the helper and returns at once.",
                "Come back here: it says Helper running. Repeat after every restart of the phone."
            ))
            add(tv(AutoCopy.adbCommand(this@SetupActivity), 11f, pal.dim, mono = true).apply { setTextIsSelectable(true) }, top = 12)
            add(secondaryButton(this@SetupActivity, pal, "Copy the command") {
                (getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager)
                    .setPrimaryClip(ClipData.newPlainText("klip helper", AutoCopy.adbCommand(this@SetupActivity)))
                Toast.makeText(this@SetupActivity, "Command copied. The text contains a secret: don't share it.", Toast.LENGTH_LONG).show()
            }, top = 10)

            add(sectionLabel(this@SetupActivity, pal, "Rooted phone"), top = 20, bottom = 8)
            add(tv("Let Klip start the helper itself, now and at every start of the phone. Your root manager will ask once.", 14f, pal.text))
            val rootOn = AutoCopy.rootEnabled(this@SetupActivity)
            add(
                if (rootOn) secondaryButton(this@SetupActivity, pal, "Turn off the root option") {
                    Thread { AutoCopy.setRootEnabled(this@SetupActivity, false) }.start(); render()
                } else primaryButton(this@SetupActivity, pal, "Start with root") {
                    Thread { AutoCopy.setRootEnabled(this@SetupActivity, true) }.start(); render()
                }, top = 10
            )

            val st = tv(helperStatusText(), 13.5f, if (running) pal.signal else pal.dim, 1)
            helperStatus = st
            add(st, top = 14)
            add(textButton(this@SetupActivity, pal, "Stop the helper now", pal.dim) {
                ClipHelperServer.stopHelper(this@SetupActivity); Toast.makeText(this@SetupActivity, "Asked the helper to stop.", Toast.LENGTH_SHORT).show()
            }, top = 2)
        }
    }

    private fun testCard(): View = item("test", "Check: can this phone receive in the background?", "A 10-second test.", null, "Test") {
        block("How",
            "Tap the button, switch to any other app, and wait ten seconds. Klip puts a test text on the clipboard. If you can paste it, background receiving works on this phone.")
        add(secondaryButton(this@SetupActivity, pal, "Start the test") {
            startService(Intent(this@SetupActivity, KlipSyncService::class.java).setAction(KlipSyncService.ACTION_TEST_BG_WRITE))
            Toast.makeText(this@SetupActivity, "Now switch to another app. In 10 seconds, paste.", Toast.LENGTH_LONG).show()
        }, top = 14)
    }

    /** Progress as N short bars. */
    private class Segments(ctx: Context, private val pal: Pal, private val total: Int, private val done: Int) : View(ctx) {
        private val p = Paint(Paint.ANTI_ALIAS_FLAG)
        private val r = RectF()
        override fun onMeasure(w: Int, h: Int) = setMeasuredDimension(MeasureSpec.getSize(w), context.dp(6))
        override fun onDraw(c: Canvas) {
            if (total == 0) return
            val gap = context.dpf(6f)
            val seg = (width - gap * (total - 1)) / total
            for (i in 0 until total) {
                p.color = if (i < done) pal.signal else pal.line
                r.set(i * (seg + gap), 0f, i * (seg + gap) + seg, height.toFloat())
                c.drawRoundRect(r, height / 2f, height / 2f, p)
            }
        }
    }
}
