package com.klip.app

import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Bundle
import android.view.Gravity
import android.view.ViewGroup
import android.widget.FrameLayout
import android.widget.LinearLayout
import com.google.zxing.BarcodeFormat
import com.journeyapps.barcodescanner.BarcodeCallback
import com.journeyapps.barcodescanner.BarcodeResult
import com.journeyapps.barcodescanner.DecoratedBarcodeView
import com.journeyapps.barcodescanner.DefaultDecoderFactory
import com.klip.app.ui.Pal
import com.klip.app.ui.dp
import com.klip.app.ui.dpf
import com.klip.app.ui.edgeToEdge
import com.klip.app.ui.rounded
import com.klip.app.ui.textButton
import com.klip.app.ui.tv
import com.klip.app.ui.withAlpha
import uniffi.klip_android.isValidPairingTicket

/** Camera view with two lines of instruction on top and an escape hatch (paste) at the bottom. */
class QrScannerActivity : Activity() {

    companion object {
        const val EXTRA_SCANNED_TICKET = "extra_scanned_ticket"
        /** Result code: the person would rather paste the ticket. */
        const val RESULT_PASTE = RESULT_FIRST_USER
        private const val REQUEST_CAMERA = 101
    }

    private lateinit var barcodeView: DecoratedBarcodeView
    private var handled = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val pal = Pal(dark = true) // camera screens are dark in both themes
        val root = FrameLayout(this).apply { setBackgroundColor(0xFF000000.toInt()) }

        barcodeView = DecoratedBarcodeView(this).apply {
            barcodeView.decoderFactory = DefaultDecoderFactory(listOf(BarcodeFormat.QR_CODE))
            statusView.text = ""
        }
        root.addView(barcodeView, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))

        val top = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(20), dp(12), dp(20), dp(16))
            background = rounded(withAlpha(0xFF000000.toInt(), 0.55f), 0f)
        }
        top.addView(tv("Scan the code", 22f, pal.text, 2))
        top.addView(tv("On your computer run  klip pair  and point the camera at the QR it shows.", 14f, pal.dim),
            LinearLayout.LayoutParams(-1, -2).apply { topMargin = dp(6) })
        root.addView(top, FrameLayout.LayoutParams(-1, -2, Gravity.TOP))

        val bottom = LinearLayout(this).apply {
            gravity = Gravity.CENTER
            setPadding(dp(20), dp(10), dp(20), dp(14))
            background = rounded(withAlpha(0xFF000000.toInt(), 0.55f), 0f)
        }
        bottom.addView(textButton(this, pal, "Paste a ticket instead", pal.text) {
            setResult(RESULT_PASTE); finish()
        })
        bottom.addView(textButton(this, pal, "Cancel", pal.dim) { setResult(RESULT_CANCELED); finish() },
            LinearLayout.LayoutParams(-2, -2).apply { leftMargin = dp(18) })
        root.addView(bottom, FrameLayout.LayoutParams(-1, -2, Gravity.BOTTOM))

        setContentView(root)
        edgeToEdge(pal, root)

        if (checkSelfPermission(android.Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) {
            startScanning()
        } else {
            requestPermissions(arrayOf(android.Manifest.permission.CAMERA), REQUEST_CAMERA)
        }
    }

    @Deprecated("Deprecated in Java")
    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        @Suppress("DEPRECATION")
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode != REQUEST_CAMERA) return
        if (grantResults.firstOrNull() == PackageManager.PERMISSION_GRANTED) {
            startScanning()
        } else {
            // No camera: the paste route still works.
            setResult(RESULT_PASTE)
            finish()
        }
    }

    private fun startScanning() {
        handled = false
        barcodeView.decodeContinuous(object : BarcodeCallback {
            override fun barcodeResult(result: BarcodeResult?) {
                val text = result?.text?.trim() ?: return
                if (handled || text.isEmpty() || !isValidPairingTicket(text)) return // keep looking
                handled = true
                barcodeView.pause()
                setResult(RESULT_OK, Intent().putExtra(EXTRA_SCANNED_TICKET, text))
                finish()
            }
        })
        barcodeView.resume()
    }

    override fun onResume() {
        super.onResume()
        if (!handled && checkSelfPermission(android.Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) {
            barcodeView.resume()
        }
    }

    override fun onPause() {
        barcodeView.pause()
        super.onPause()
    }
}
