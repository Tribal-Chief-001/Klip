package com.klip.app

import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Bundle
import android.util.Log
import android.widget.Button
import android.widget.Toast
import androidx.appcompat.app.AppCompatActivity
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import com.google.zxing.BarcodeFormat
import com.journeyapps.barcodescanner.BarcodeCallback
import com.journeyapps.barcodescanner.BarcodeResult
import com.journeyapps.barcodescanner.DecoratedBarcodeView
import com.journeyapps.barcodescanner.DefaultDecoderFactory

class QrScannerActivity : AppCompatActivity() {

    companion object {
        const val EXTRA_SCANNED_TOKEN = "extra_scanned_token"
        private const val REQUEST_CAMERA_PERMISSION = 101
        private const val TAG = "QrScannerActivity"
    }

    private lateinit var barcodeView: DecoratedBarcodeView
    private lateinit var btnCancel: Button
    private var isScanHandled = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_qr_scan)

        barcodeView = findViewById(R.id.barcodeScannerView)
        btnCancel = findViewById(R.id.btnCancelScan)

        // Configure scanner specifically for QR codes
        val formats = listOf(BarcodeFormat.QR_CODE)
        barcodeView.barcodeView.decoderFactory = DefaultDecoderFactory(formats)
        barcodeView.statusView.text = "Align QR code within frame"

        btnCancel.setOnClickListener {
            setResult(Activity.RESULT_CANCELED)
            finish()
        }

        if (ContextCompat.checkSelfPermission(this, android.Manifest.permission.CAMERA)
            != PackageManager.PERMISSION_GRANTED
        ) {
            ActivityCompat.requestPermissions(
                this,
                arrayOf(android.Manifest.permission.CAMERA),
                REQUEST_CAMERA_PERMISSION
            )
        } else {
            startScanning()
        }
    }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode == REQUEST_CAMERA_PERMISSION) {
            if (grantResults.isNotEmpty() && grantResults[0] == PackageManager.PERMISSION_GRANTED) {
                startScanning()
            } else {
                Toast.makeText(this, "Camera permission denied. Use manual token input.", Toast.LENGTH_SHORT).show()
                finish()
            }
        }
    }

    private fun startScanning() {
        isScanHandled = false
        barcodeView.decodeContinuous(object : BarcodeCallback {
            override fun barcodeResult(result: BarcodeResult?) {
                if (result == null || isScanHandled) return
                val text = result.text?.trim() ?: return
                if (text.isNotEmpty()) {
                    isScanHandled = true
                    barcodeView.pause()
                    Log.i(TAG, "QR Code decoded successfully: ${text.take(32)}...")

                    val data = Intent().apply {
                        putExtra(EXTRA_SCANNED_TOKEN, text)
                    }
                    setResult(Activity.RESULT_OK, data)
                    finish()
                }
            }
        })
        barcodeView.resume()
    }

    override fun onResume() {
        super.onResume()
        if (ContextCompat.checkSelfPermission(this, android.Manifest.permission.CAMERA)
            == PackageManager.PERMISSION_GRANTED && !isScanHandled
        ) {
            barcodeView.resume()
        }
    }

    override fun onPause() {
        super.onPause()
        barcodeView.pause()
    }

    override fun onDestroy() {
        super.onDestroy()
        barcodeView.pause()
    }
}
