package com.klip.app

import android.app.Activity
import android.content.Intent
import android.content.res.ColorStateList
import android.graphics.drawable.GradientDrawable
import android.os.Bundle
import android.view.Gravity
import android.view.View
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.Switch
import android.widget.TextView
import com.klip.app.ui.LinkView
import com.klip.app.ui.Page
import com.klip.app.ui.Pal
import com.klip.app.ui.Sheet
import com.klip.app.ui.add
import com.klip.app.ui.card
import com.klip.app.ui.column
import com.klip.app.ui.divider
import com.klip.app.ui.dot
import com.klip.app.ui.dp
import com.klip.app.ui.dpf
import com.klip.app.ui.lp
import com.klip.app.ui.primaryButton
import com.klip.app.ui.rippleOver
import com.klip.app.ui.rounded
import com.klip.app.ui.secondaryButton
import com.klip.app.ui.sectionLabel
import com.klip.app.ui.textButton
import com.klip.app.ui.tv
import com.klip.app.ui.withAlpha
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.klip_android.PeerInfo
import uniffi.klip_android.StatusInfo

/** Home: is Klip connected, one big "send" button, your devices, what moved recently. */
class MainActivity : Activity() {

    private companion object {
        const val REQ_SCAN = 7
        const val REQ_FOLDER = 8
    }

    private val scope = MainScope()
    private var poll: Job? = null
    private lateinit var pal: Pal
    private lateinit var link: LinkView
    private lateinit var heroTitle: TextView
    private lateinit var heroSub: TextView
    private lateinit var banner: LinearLayout
    private lateinit var bannerSub: TextView
    private lateinit var setupBadge: View
    private lateinit var devices: LinearLayout
    private lateinit var recent: LinearLayout
    private lateinit var pauseSub: TextView
    private lateinit var pauseChip: TextView
    private lateinit var footer: TextView
    private lateinit var pairing: Pairing
    private var devicesKey = ""
    private var recentKey = ""

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        pal = Pal.of(this)
        pairing = Pairing(this, pal, scope, scanScanner = {
            @Suppress("DEPRECATION")
            startActivityForResult(Intent(this, QrScannerActivity::class.java), REQ_SCAN)
        }, onLinked = { scope.launch { refresh() } })
        build()
        KlipSyncService.start(this)
    }

    // ------------------------------------------------------------------------------------ layout

    private fun build() {
        val page = Page(this, pal)
        val b = page.body

        // Header: wordmark on the left, two quiet links on the right.
        val header = LinearLayout(this).apply { gravity = Gravity.CENTER_VERTICAL }
        header.addView(ImageView(this).apply {
            setImageResource(R.drawable.ic_klip_logo)
            imageTintList = ColorStateList.valueOf(pal.signal)
        }, LinearLayout.LayoutParams(dp(26), dp(26)))
        header.addView(tv("klip", 24f, pal.text, 2).apply { letterSpacing = -0.02f },
            LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f).apply { leftMargin = dp(8) })
        val setupLink = LinearLayout(this).apply {
            gravity = Gravity.CENTER_VERTICAL
            isClickable = true
            setOnClickListener { startActivity(Intent(this@MainActivity, SetupActivity::class.java)) }
        }
        setupBadge = dot(this, pal.warn, 8).apply { visibility = View.GONE }
        setupLink.addView(textButton(this, pal, "Setup", pal.text) {
            startActivity(Intent(this, SetupActivity::class.java))
        })
        setupLink.addView(setupBadge, LinearLayout.LayoutParams(dp(8), dp(8)).apply { rightMargin = dp(10) })
        header.addView(setupLink)
        header.addView(textButton(this, pal, "Privacy", pal.text) {
            startActivity(Intent(this, PrivacyActivity::class.java))
        })
        b.add(header, bottom = 14)

        // Hero: the live link and one sentence about it.
        val hero = card(this, pal, 18)
        link = LinkView(this, pal)
        hero.addView(link, lp(h = LinearLayout.LayoutParams.WRAP_CONTENT))
        heroTitle = tv("Starting…", 21f, pal.text, 2)
        heroSub = tv("", 14f, pal.dim)
        hero.add(heroTitle, top = 6)
        hero.add(heroSub, top = 5)
        b.add(hero, bottom = 12)

        // Reliability banner: only visible while something is missing.
        banner = column(this).apply {
            visibility = View.GONE
            setPadding(dp(16), dp(14), dp(16), dp(14))
            rippleOver(pal, rounded(withAlpha(pal.warn, 0.10f), dpf(18f), withAlpha(pal.warn, 0.45f), 1), dpf(18f))
            isClickable = true
            setOnClickListener { startActivity(Intent(this@MainActivity, SetupActivity::class.java)) }
        }
        banner.add(tv("Finish setup so Klip keeps running", 15f, pal.text, 1))
        bannerSub = tv("", 13.5f, pal.dim)
        banner.add(bannerSub, top = 4)
        b.add(banner, bottom = 12)

        b.add(primaryButton(this, pal, "Send clipboard now") {
            startActivity(
                Intent(this, CopyTrampolineActivity::class.java)
                    .setAction(CopyTrampolineActivity.ACTION_SEND_CLIPBOARD)
            )
        })
        b.add(
            tv("Copy something first. You can also use Share → Klip, the Quick Settings tile, or the button in Klip's notification.", 12.5f, pal.dim),
            top = 8, bottom = 22
        )

        // Folders: pick one with the system's picker and send it to the linked devices.
        b.add(secondaryButton(this, pal, "Send a folder") {
            @Suppress("DEPRECATION")
            startActivityForResult(
                Intent(Intent.ACTION_OPEN_DOCUMENT_TREE).addFlags(
                    Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION
                ),
                REQ_FOLDER
            )
        }, bottom = 22)

        // Devices.
        b.add(sectionLabel(this, pal, "Devices"), bottom = 10)
        devices = column(this)
        b.add(devices, bottom = 10)
        b.add(secondaryButton(this, pal, "Add a device") { pairing.showAddSheet() }, bottom = 26)

        // Recent.
        b.add(sectionLabel(this, pal, "Recent"), bottom = 10)
        recent = column(this)
        b.add(recent, bottom = 26)

        // Pause: tap to choose how long (or until you turn it back on).
        val pauseRow = LinearLayout(this).apply {
            gravity = Gravity.CENTER_VERTICAL
            setPadding(0, dp(6), 0, dp(6))
            isClickable = true
            setOnClickListener { showPauseSheet() }
        }
        val pauseText = column(this)
        pauseText.add(tv("Pause syncing", 15f, pal.text, 1))
        pauseSub = tv("", 12.5f, pal.dim)
        pauseText.add(pauseSub, top = 2)
        pauseRow.addView(pauseText, LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f))
        pauseChip = com.klip.app.ui.chip(this, "Off", pal.dim)
        pauseRow.addView(pauseChip)
        b.add(pauseRow, bottom = 22)

        footer = tv("", 11.5f, pal.faint, mono = true)
        b.add(footer)
    }

    // ---------------------------------------------------------------------------------- lifecycle

    override fun onStart() {
        super.onStart()
        KlipSyncService.start(this) // idempotent; also restores the notification if it went missing
        noticeIfStopped()
        poll = scope.launch {
            while (isActive) {
                refresh()
                delay(2000)
            }
        }
    }

    override fun onStop() {
        poll?.cancel()
        super.onStop()
    }

    override fun onDestroy() {
        scope.cancel()
        super.onDestroy()
    }

    @Deprecated("Deprecated in Java")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        @Suppress("DEPRECATION")
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode == REQ_FOLDER) {
            if (resultCode == RESULT_OK) data?.data?.let { FolderSender.start(this, it) }
            return
        }
        if (requestCode != REQ_SCAN) return
        when (resultCode) {
            RESULT_OK -> data?.getStringExtra(QrScannerActivity.EXTRA_SCANNED_TICKET)?.let { pairing.join(it) }
            QrScannerActivity.RESULT_PASTE -> pairing.showPaste(null)
        }
    }

    /** Tell the person once if Android's active-apps "Stop" (or Force stop) had ended Klip. */
    private fun noticeIfStopped() {
        if (android.os.Build.VERSION.SDK_INT < 30) return
        val last = runCatching {
            getSystemService(android.app.ActivityManager::class.java).getHistoricalProcessExitReasons(packageName, 0, 1).firstOrNull()
        }.getOrNull() ?: return
        val prefs = getSharedPreferences("klip_ui", MODE_PRIVATE)
        if (last.reason == android.app.ApplicationExitInfo.REASON_USER_REQUESTED && last.timestamp > prefs.getLong("stop_seen", 0)) {
            prefs.edit().putLong("stop_seen", last.timestamp).apply()
            android.widget.Toast.makeText(this, "Klip had been stopped from Android's app list. It is running again.", android.widget.Toast.LENGTH_LONG).show()
        }
    }

    // ------------------------------------------------------------------------------------- status

    private suspend fun refresh() {
        val open = Setup.openCount(this)
        banner.visibility = if (open > 0) View.VISIBLE else View.GONE
        setupBadge.visibility = if (open > 0) View.VISIBLE else View.GONE
        if (open > 0) bannerSub.text = "$open ${if (open == 1) "step" else "steps"} left. Your phone may stop Klip in the background otherwise."

        val engine = KlipRuntime.engine
        if (engine == null) {
            heroTitle.text = "Starting…"
            heroSub.text = ""
            return
        }
        val st = withContext(Dispatchers.IO) { runCatching { engine.status() }.getOrNull() }
        val history = withContext(Dispatchers.IO) { runCatching { engine.history(5u) }.getOrDefault(emptyList()) }
        if (st == null) {
            heroTitle.text = "Klip isn't responding"
            heroSub.text = "Close and reopen the app."
            return
        }
        renderHero(st)
        renderDevices(st)
        renderRecent(history)
        renderPause(st.paused)
        footer.text = "klip ${versionName()} · ${st.deviceFingerprint}"
    }

    private fun versionName(): String =
        runCatching { packageManager.getPackageInfo(packageName, 0).versionName }.getOrNull() ?: ""

    private fun renderPause(paused: Boolean) {
        val until = KlipRuntime.pauseUntilMs(this)
        if (!paused) {
            pauseSub.text = "Tap to choose for how long."
            pauseChip.text = "Off"
            pauseChip.background = com.klip.app.ui.rounded(com.klip.app.ui.withAlpha(pal.dim, 0.14f), dpf(20f))
            pauseChip.setTextColor(pal.dim)
        } else {
            pauseSub.text = when {
                until == KlipRuntime.PAUSE_FOREVER -> "Paused until you turn it back on."
                until > 0 -> "Paused until " + java.text.DateFormat.getTimeInstance(java.text.DateFormat.SHORT).format(java.util.Date(until)) + "."
                else -> "Paused."
            }
            pauseChip.text = "Paused"
            pauseChip.background = com.klip.app.ui.rounded(com.klip.app.ui.withAlpha(pal.warn, 0.14f), dpf(20f))
            pauseChip.setTextColor(pal.warn)
        }
    }

    private fun showPauseSheet() {
        val paused = KlipRuntime.pauseUntilMs(this) != 0L
        val s = Sheet(this, pal)
        if (paused) {
            s.title("Syncing is paused", "Nothing is sent or received until it ends.")
            s.body.add(primaryButton(this, pal, "Resume now") {
                s.dismiss()
                scope.launch { withContext(Dispatchers.IO) { KlipRuntime.resume(this@MainActivity) }; refresh() }
            }, bottom = 10)
            s.body.add(sectionLabel(this, pal, "Or pause for"), top = 8, bottom = 8)
        } else {
            s.title("Pause syncing", "Nothing is sent or received while paused. Devices stay linked.")
        }
        val choices = listOf(
            "15 minutes" to 15L * 60, "1 hour" to 3600L, "8 hours" to 8L * 3600, "Until I turn it back on" to null
        )
        for ((label, secs) in choices) {
            s.body.add(secondaryButton(this, pal, label) {
                s.dismiss()
                scope.launch { withContext(Dispatchers.IO) { KlipRuntime.pause(this@MainActivity, secs) }; refresh() }
            }, bottom = 8)
        }
        s.show()
    }

    private fun renderHero(st: StatusInfo) {
        link.set(st.peers.map { LinkView.Node(it.name, it.online) })
        val online = st.peers.filter { it.online }
        val paused = if (st.paused) "Paused. " else ""
        when {
            st.peers.isEmpty() -> {
                heroTitle.text = "Link your first device"
                heroSub.text = "Add your computer or another phone to start sharing the clipboard."
            }
            online.isNotEmpty() -> {
                heroTitle.text = if (online.size == 1) "Connected to ${online[0].name}" else "Connected to ${online.size} devices"
                val last = st.lastClipPreview
                heroSub.text = paused + if (last != null && st.lastSyncSecsAgo != null) {
                    "Last clip ${Fmt.ago(st.lastSyncSecsAgo!!.toLong())}."
                } else "Copy on one device, paste on the other."
            }
            else -> {
                heroTitle.text = if (st.peers.size == 1) "Waiting for ${st.peers[0].name}" else "Waiting for your devices"
                heroSub.text = paused + offlineHint(st)
            }
        }
    }

    /** Why nothing is connected, in words a person can act on. */
    private fun offlineHint(st: StatusInfo): String {
        st.peers.firstNotNullOfOrNull { it.lastError }?.let { return Fmt.plainPeerError(it) }
        if (st.relayMode.contains("LAN", true).not() && st.relayState.contains("unreachable", true)) {
            return "No route to the relay server. Devices on the same Wi-Fi can still connect."
        }
        return "Make sure Klip is running on the other device. Reconnects happen by themselves."
    }

    private fun renderDevices(st: StatusInfo) {
        val key = st.peers.joinToString("|") { "${it.fingerprint}:${it.online}:${it.lastError}" }
        if (key == devicesKey) return
        devicesKey = key
        devices.removeAllViews()
        if (st.peers.isEmpty()) {
            devices.add(tv("None yet.", 14f, pal.dim))
            return
        }
        val box = card(this, pal, 0)
        st.peers.forEachIndexed { i, p ->
            if (i > 0) box.addView(divider(this, pal))
            box.addView(deviceRow(p), lp())
        }
        devices.addView(box, lp())
    }

    private fun deviceRow(p: PeerInfo): View {
        val row = LinearLayout(this).apply {
            gravity = Gravity.CENTER_VERTICAL
            setPadding(dp(16), dp(14), dp(16), dp(14))
            rippleOver(pal, null, dpf(18f))
            isClickable = true
            setOnClickListener { showDevice(p) }
        }
        row.addView(dot(this, if (p.online) pal.signal else pal.faint, 10, hollow = !p.online),
            LinearLayout.LayoutParams(dp(10), dp(10)).apply { rightMargin = dp(14) })
        val text = column(this)
        text.add(tv(p.name, 15.5f, pal.text, 1, lines = 1))
        val state = when {
            p.online -> "connected"
            p.lastError != null -> Fmt.plainPeerError(p.lastError!!)
            else -> "offline"
        }
        text.add(tv(state, 12.5f, if (p.online) pal.signal else pal.dim, lines = 1), top = 3)
        row.addView(text, LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f))
        row.addView(tv("›", 22f, pal.faint))
        return row
    }

    private fun showDevice(p: PeerInfo) {
        val s = Sheet(this, pal)
        s.title(p.name, if (p.online) "Connected right now." else "Not connected right now.")
        s.body.add(sectionLabel(this, pal, "Fingerprint"), bottom = 6)
        s.body.add(tv(p.fingerprint, 18f, pal.text, 1, mono = true))
        s.body.add(
            tv("It should match the fingerprint that device shows for itself. If it ever doesn't, remove the device.", 13f, pal.dim),
            top = 6, bottom = 14
        )
        p.lastError?.let {
            s.body.add(card(this, pal).also { c -> c.add(tv(Fmt.plainPeerError(it), 13.5f, pal.warn)) }, bottom = 14)
        }
        s.body.add(secondaryButton(this, pal, "Remove this device") {
            s.dismiss(); confirmRemove(p)
        })
        s.show()
    }

    private fun confirmRemove(p: PeerInfo) {
        val s = Sheet(this, pal)
        s.title("Remove ${p.name}?", "Klip stops sharing with it right away. To link it again you will need a new code.")
        s.body.add(primaryButton(this, pal, "Remove") {
            s.dismiss()
            val e = KlipRuntime.engine ?: return@primaryButton
            scope.launch {
                withContext(Dispatchers.IO) { runCatching { e.unpair(p.fingerprint) } }
                refresh()
            }
        }, bottom = 10)
        s.body.add(secondaryButton(this, pal, "Keep it") { s.dismiss() })
        s.show()
    }

    private fun renderRecent(items: List<uniffi.klip_android.HistoryEntry>) {
        val key = items.joinToString("|") { "${it.timestampMs}:${it.preview}" }
        // The "x min ago" text changes with time, so rebuild at most every refresh anyway; cheap.
        if (key == recentKey && items.isNotEmpty() && recent.childCount > 0) {
            updateAgo(items)
            return
        }
        recentKey = key
        recent.removeAllViews()
        if (items.isEmpty()) {
            recent.add(tv("Nothing yet. Clips you send or receive show up here.", 14f, pal.dim))
            return
        }
        val box = card(this, pal, 0)
        items.forEachIndexed { i, h ->
            if (i > 0) box.addView(divider(this, pal))
            val row = column(this).apply { setPadding(dp(16), dp(12), dp(16), dp(12)) }
            val preview = if (h.sensitive) "•••• hidden: looks like a secret" else h.preview.replace('\n', ' ')
            row.add(tv(preview, 13.5f, if (h.sensitive) pal.warn else pal.text, mono = !h.sensitive, lines = 1))
            val sub = tv("", 12f, pal.dim)
            sub.tag = h.timestampMs
            row.add(sub, top = 4)
            box.addView(row, lp())
        }
        recent.addView(box, lp())
        updateAgo(items)
    }

    private fun updateAgo(items: List<uniffi.klip_android.HistoryEntry>) {
        val box = recent.getChildAt(0) as? LinearLayout ?: return
        var idx = 0
        for (c in 0 until box.childCount) {
            val row = box.getChildAt(c) as? LinearLayout ?: continue
            if (row.childCount < 2 || idx >= items.size) continue
            val h = items[idx++]
            (row.getChildAt(1) as? TextView)?.text =
                "${if (h.fromDevice.isBlank()) "this phone" else h.fromDevice} · ${Fmt.agoMs(h.timestampMs.toLong())}"
        }
    }
}
