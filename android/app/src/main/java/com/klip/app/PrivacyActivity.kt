package com.klip.app

import android.app.Activity
import android.os.Bundle
import android.view.Gravity
import android.view.View
import android.widget.LinearLayout
import android.widget.Switch
import android.content.res.ColorStateList
import com.klip.app.ui.Page
import com.klip.app.ui.Pal
import com.klip.app.ui.add
import com.klip.app.ui.card
import com.klip.app.ui.column
import com.klip.app.ui.divider
import com.klip.app.ui.dp
import com.klip.app.ui.sectionLabel
import com.klip.app.ui.textButton
import com.klip.app.ui.tv
import com.klip.app.ui.withAlpha

/** What Klip does with your data and your permissions, in plain words. Nothing here is marketing. */
class PrivacyActivity : Activity() {

    private lateinit var pal: Pal

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        pal = Pal.of(this)
        val b = Page(this, pal).body

        val top = LinearLayout(this).apply { gravity = Gravity.CENTER_VERTICAL }
        top.addView(textButton(this, pal, "‹ Back", pal.text) { finish() })
        b.add(top, bottom = 8)
        b.add(tv("What Klip does", 28f, pal.text, 2), bottom = 8)
        b.add(tv("Short and checkable. The code is open source: github.com/Tribal-Chief-001/Klip", 14.5f, pal.dim), bottom = 20)

        section(b, "Where your clips go",
            "Only to the devices you linked yourself, directly or through a relay (below). There is no Klip account and no Klip server that stores anything. " +
                "Each device has its own key. Linking needs a one-time code and an approval on the first device. Text is encrypted on its way (QUIC with TLS 1.3, the key of each device).")

        // Relay switch with its explanation.
        val relay = card(this, pal)
        val row = LinearLayout(this).apply { gravity = Gravity.CENTER_VERTICAL }
        val t = column(this)
        t.add(tv("Use relay servers", 16f, pal.text, 1))
        t.add(tv("Lets devices on different networks find each other.", 13.5f, pal.dim), top = 3)
        row.addView(t, LinearLayout.LayoutParams(0, -2, 1f))
        row.addView(Switch(this).apply {
            isChecked = KlipRuntime.useRelays(this@PrivacyActivity)
            thumbTintList = ColorStateList(arrayOf(intArrayOf(android.R.attr.state_checked), intArrayOf()), intArrayOf(pal.signal, pal.faint))
            trackTintList = ColorStateList(arrayOf(intArrayOf(android.R.attr.state_checked), intArrayOf()), intArrayOf(withAlpha(pal.signal, 0.35f), pal.line))
            setOnCheckedChangeListener { _, on ->
                if (on != KlipRuntime.useRelays(this@PrivacyActivity)) {
                    KlipRuntime.setUseRelays(this@PrivacyActivity, on)
                    KlipSyncService.restart(this@PrivacyActivity)
                }
            }
        })
        relay.addView(row)
        relay.add(
            tv("Relays are public servers run by n0 (the iroh project). A relay can see which devices connect and when. It cannot read what you copy. " +
                "Turn this off to work on the same Wi-Fi only, with no third-party server at all.", 13.5f, pal.dim), top = 12
        )
        b.add(relay, bottom = 22)

        // Restart after Android's "Stop" button.
        val keep = card(this, pal)
        val krow = LinearLayout(this).apply { gravity = Gravity.CENTER_VERTICAL }
        val kt = column(this)
        kt.add(tv("Restart by itself if stopped", 16f, pal.text, 1))
        kt.add(tv("Checks every 15 minutes or so.", 13.5f, pal.dim), top = 3)
        krow.addView(kt, LinearLayout.LayoutParams(0, -2, 1f))
        krow.addView(Switch(this).apply {
            isChecked = Watchdog.enabled(this@PrivacyActivity)
            thumbTintList = ColorStateList(arrayOf(intArrayOf(android.R.attr.state_checked), intArrayOf()), intArrayOf(pal.signal, pal.faint))
            trackTintList = ColorStateList(arrayOf(intArrayOf(android.R.attr.state_checked), intArrayOf()), intArrayOf(withAlpha(pal.signal, 0.35f), pal.line))
            setOnCheckedChangeListener { _, on -> Watchdog.setEnabled(this@PrivacyActivity, on) }
        })
        keep.addView(krow)
        keep.add(
            tv("Android's list of active apps has a Stop button that removes Klip from memory. With this on, Klip starts itself again " +
                "within about 15 minutes, provided Unrestricted battery use is allowed in Setup. A Force stop in App info cannot be " +
                "undone by any app: open Klip once to start it again. Turn this off if you want Stop to mean stop.", 13.5f, pal.dim), top = 12
        )
        b.add(keep, bottom = 22)

        section(b, "What Klip reads",
            "The clipboard, and only when you ask (Send button, Share menu, the tile, the notification button) or while Klip is on screen. " +
                "Android does not allow more, unless you start the optional helper (Setup → Automatic copy), which then also sees what you copy in other apps, " +
                "except what an app marks as sensitive. Text that arrives from your other devices is written to the clipboard.")
        section(b, "What Klip never does",
            "No account, no ads, no analytics, no tracking. No copy of your clips is kept anywhere except on your own devices. " +
                "No access to your contacts, messages, photos or files. The camera is used only while the QR scanner is open.")
        section(b, "Passwords and secrets",
            "Text that looks like a key or password still syncs like any other text. Klip hides it in its Recent list and marks it " +
                "“sensitive” so keyboards don't preview it. The detector can be wrong in both directions.")

        b.add(sectionLabel(this, pal, "Permissions"), bottom = 10)
        val perms = card(this, pal, 0)
        val rows = listOf(
            "Internet" to "Talk to your devices and the relay.",
            "Network state, Wi-Fi multicast" to "Find your computer on the same Wi-Fi.",
            "Notifications" to "Show that Klip is running, with a Send button.",
            "Foreground service" to "Keep the connection open in the background.",
            "Camera" to "Scan the pairing QR code. Nothing is recorded or saved.",
            "Run at startup" to "Start Klip again after the phone restarts.",
            "Ignore battery optimisation" to "Only asks; you decide in Setup.",
            "Nothing else by default" to "No log access, no “display over other apps”, no accessibility service. The optional clipboard helper (Setup → Automatic copy) is a separate process you start yourself."
        )
        rows.forEachIndexed { i, (name, why) ->
            if (i > 0) perms.addView(divider(this, pal))
            val r = column(this).apply { setPadding(dp(16), dp(12), dp(16), dp(12)) }
            r.add(tv(name, 14.5f, pal.text, 1))
            r.add(tv(why, 13f, pal.dim), top = 3)
            perms.addView(r)
        }
        b.add(perms, bottom = 22)

        b.add(sectionLabel(this, pal, "This device"), bottom = 8)
        val fp = KlipRuntime.engine?.let { runCatching { it.deviceFingerprint() }.getOrNull() } ?: "starting…"
        b.add(tv(fp, 18f, pal.text, 1, mono = true))
        b.add(tv("Other devices show this when you link, so you can check you are linking the right one.", 13f, pal.dim), top = 6, bottom = 8)
    }

    private fun section(b: LinearLayout, title: String, text: String) {
        b.add(sectionLabel(this, pal, title), bottom = 8)
        b.add(tv(text, 14.5f, pal.text), bottom = 22)
    }
}
