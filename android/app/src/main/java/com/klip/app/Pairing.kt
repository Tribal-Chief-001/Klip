package com.klip.app

import android.app.Activity
import android.content.ClipboardManager
import android.content.Context
import android.graphics.Typeface
import android.text.InputType
import android.view.Gravity
import android.view.View
import android.widget.EditText
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.ProgressBar
import android.widget.TextView
import com.google.zxing.BarcodeFormat
import com.journeyapps.barcodescanner.BarcodeEncoder
import com.klip.app.ui.Pal
import com.klip.app.ui.Sheet
import com.klip.app.ui.add
import com.klip.app.ui.card
import com.klip.app.ui.column
import com.klip.app.ui.dp
import com.klip.app.ui.dpf
import com.klip.app.ui.lp
import com.klip.app.ui.primaryButton
import com.klip.app.ui.rippleOver
import com.klip.app.ui.rounded
import com.klip.app.ui.secondaryButton
import com.klip.app.ui.textButton
import com.klip.app.ui.tv
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.klip_android.isValidPairingTicket

/** Everything about linking a device: the three ways in, the code, the approval, plain-language errors. */
class Pairing(
    private val activity: Activity,
    private val pal: Pal,
    private val scope: CoroutineScope,
    private val scanScanner: () -> Unit,
    private val onLinked: () -> Unit
) {
    private var sheet: Sheet? = null

    private fun open(): Sheet {
        sheet?.dismiss()
        return Sheet(activity, pal).also { sheet = it }
    }

    // ------------------------------------------------------------------------ the three ways in

    fun showAddSheet() {
        val s = open()
        s.title("Add a device", "Pick whichever suits the other device. It takes about ten seconds.")
        option(s, "Scan a code", "The other device shows a QR code. On a computer run  klip pair  to get one.") {
            s.dismiss(); scanScanner()
        }
        option(s, "Show my code", "Let another phone scan a QR code from this phone.") {
            s.dismiss(); showMyCode()
        }
        option(s, "Paste a ticket", "Copy the text that  klip pair  prints and paste it here.") {
            s.dismiss(); showPaste(null)
        }
        s.show()
    }

    private fun option(s: Sheet, title: String, sub: String, onClick: () -> Unit) {
        val row = column(activity).apply {
            setPadding(activity.dp(16), activity.dp(14), activity.dp(16), activity.dp(14))
            rippleOver(pal, rounded(pal.surface2, activity.dpf(16f), pal.line, 1), activity.dpf(16f))
            isClickable = true
            setOnClickListener { onClick() }
        }
        row.add(activity.tv(title, 16f, pal.text, 1))
        row.add(activity.tv(sub, 13.5f, pal.dim), top = 4)
        s.body.add(row, bottom = 10)
    }

    // ------------------------------------------------------------------------------- join (paste)

    fun showPaste(prefill: String?) {
        val s = open()
        s.title("Paste the ticket", "Run  klip pair  on the computer, copy the ticket it prints (starts with klip1), and paste it here.")
        val input = EditText(activity).apply {
            hint = "klip1…"
            setHintTextColor(pal.faint)
            setTextColor(pal.text)
            typeface = Typeface.MONOSPACE
            textSize = 13f
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_MULTI_LINE or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
            minLines = 3
            maxLines = 6
            gravity = Gravity.TOP
            setPadding(activity.dp(14), activity.dp(12), activity.dp(14), activity.dp(12))
            background = rounded(pal.surface2, activity.dpf(14f), pal.line, 1)
            val clip = (activity.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager)
                .primaryClip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.text?.toString()
            val start = prefill ?: clip?.takeIf { isValidPairingTicket(it.trim()) }
            if (start != null) setText(start.trim())
        }
        s.body.add(input, bottom = 6)
        val note = activity.tv("", 13f, pal.bad)
        s.body.add(note, bottom = 10)
        s.body.add(primaryButton(activity, pal, "Link device") {
            val t = input.text.toString().trim()
            if (!isValidPairingTicket(t)) {
                note.text = "That doesn't look like a Klip ticket. It starts with klip1 and is one long line."
            } else {
                s.dismiss(); join(t)
            }
        })
        s.show()
    }

    // -------------------------------------------------------------------------------------- join

    fun join(ticket: String) {
        val engine = KlipRuntime.engine ?: run {
            showMessage("Klip is still starting", "Give it a few seconds and try again.")
            return
        }
        if (!isValidPairingTicket(ticket.trim())) {
            showMessage("Not a Klip code", "That code isn't a Klip ticket. Show a new one on the other device.")
            return
        }
        val s = open()
        s.cancelable(false)
        s.title("Linking…", "Approve this phone on the other device when it asks.")
        s.body.add(ProgressBar(activity).apply { isIndeterminate = true }, top = 4, bottom = 8)
        s.show()
        scope.launch {
            val outcome = withContext(Dispatchers.IO) { runCatching { engine.pairJoin(ticket.trim()) } }
            s.dismiss()
            outcome.onSuccess {
                showMessage("Linked with ${it.name}", "Klip will now share your clipboard with it.\nFingerprint ${it.fingerprint}")
                onLinked()
            }.onFailure {
                showMessage("Could not link", plainReason(it.message), retry = true)
            }
        }
    }

    // --------------------------------------------------------------------------------- show code

    fun showMyCode() {
        val engine = KlipRuntime.engine ?: run {
            showMessage("Klip is still starting", "Give it a few seconds and try again.")
            return
        }
        val s = open()
        s.title("Your code", "On the other device: Klip → Add a device → Scan a code.")
        val qrBox = LinearLayout(activity).apply {
            gravity = Gravity.CENTER
            background = rounded(0xFFFFFFFF.toInt(), activity.dpf(20f))
            setPadding(activity.dp(14), activity.dp(14), activity.dp(14), activity.dp(14))
        }
        val image = ImageView(activity)
        qrBox.addView(image, LinearLayout.LayoutParams(activity.dp(250), activity.dp(250)))
        s.body.add(qrBox.also { it.layoutParams = lp().apply { gravity = Gravity.CENTER_HORIZONTAL } })
        val caption = activity.tv("Preparing…", 13.5f, pal.dim).apply { gravity = Gravity.CENTER }
        s.body.add(caption, top = 12, bottom = 10)
        s.body.add(secondaryButton(activity, pal, "Close") { s.dismiss() })

        var watcher: Job? = null
        s.onDismiss {
            watcher?.cancel()
            scope.launch(Dispatchers.IO) { runCatching { engine.pairCancel() } }
        }
        s.show()

        watcher = scope.launch {
            val ticket = withContext(Dispatchers.IO) {
                runCatching { engine.pairStart(120UL, !KlipRuntime.useRelays(activity)) }
            }.getOrElse {
                caption.text = "Could not create a code: ${plainReason(it.message)}"
                return@launch
            }
            image.setImageBitmap(BarcodeEncoder().encodeBitmap(ticket.ticket, BarcodeFormat.QR_CODE, 720, 720))
            var asked = false
            while (isActive) {
                val p = withContext(Dispatchers.IO) { runCatching { engine.pairPoll() }.getOrNull() }
                if (p != null) {
                    p.pairedDevice?.let {
                        s.dismiss()
                        showMessage("Linked with $it", "Klip will now share your clipboard with it.")
                        onLinked()
                        return@launch
                    }
                    val pending = p.pending
                    if (pending != null && !asked) {
                        asked = true
                        s.dismiss()
                        approve(engine, pending.deviceName, pending.fingerprint)
                        return@launch
                    }
                    if (!p.windowOpen && !asked) {
                        s.dismiss()
                        showMessage("The code expired", "Codes last two minutes. Open “Show my code” again.")
                        return@launch
                    }
                    caption.text = "Valid for ${p.secondsLeft}s · this phone ${ticket.deviceFingerprint}"
                }
                delay(500)
            }
        }
    }

    private fun approve(engine: uniffi.klip_android.KlipEngine, name: String, fingerprint: String) {
        val s = open()
        s.cancelable(false)
        s.title("Link “$name”?", "Only approve a device you are setting up right now. Compare this fingerprint with the one on that device.")
        s.body.add(card(activity, pal).also {
            it.add(activity.tv(fingerprint, 20f, pal.text, 1, mono = true).apply { gravity = Gravity.CENTER })
        }, bottom = 14)
        s.body.add(primaryButton(activity, pal, "Approve") {
            s.dismiss()
            scope.launch {
                withContext(Dispatchers.IO) { runCatching { engine.pairDecide(true) } }
                onLinked()
            }
        }, bottom = 10)
        s.body.add(secondaryButton(activity, pal, "Decline") {
            s.dismiss()
            scope.launch(Dispatchers.IO) { runCatching { engine.pairDecide(false) } }
        })
        s.show()
    }

    // -------------------------------------------------------------------------------- utilities

    fun showMessage(title: String, text: String, retry: Boolean = false) {
        val s = open()
        s.title(title, text)
        if (retry) s.body.add(primaryButton(activity, pal, "Try again") { s.dismiss(); showAddSheet() }, bottom = 10)
        s.body.add(if (retry) secondaryButton(activity, pal, "Close") { s.dismiss() }
        else primaryButton(activity, pal, "Done") { s.dismiss() })
        s.show()
    }

    private fun plainReason(raw: String?): String {
        val r = raw ?: return "Unknown error."
        return when {
            r.contains("expired", true) -> "That code has expired. Show a new one on the other device."
            r.contains("declin", true) -> "The other device declined the link."
            r.contains("invalid pairing code", true) || r.contains("rejected", true) ->
                "The other device refused the code. It may have been used already: show a new one."
            r.contains("no pairing in progress", true) -> "The other device isn't showing a code right now. Open one there first."
            r.contains("timed out", true) || r.contains("unreachable", true) || r.contains("connect", true) ->
                "Couldn't reach the other device. Check that both are online (or on the same Wi-Fi) and that Klip is running there."
            else -> r
        }
    }
}
