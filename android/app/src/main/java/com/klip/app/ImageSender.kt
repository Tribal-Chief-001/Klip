package com.klip.app

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Matrix
import android.media.ExifInterface
import android.net.Uri
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.IOException
import kotlin.math.max

/**
 * Gets an image from another app ready for the linked devices. The wire format carries PNG and WebP
 * only, so everything else (JPEG, HEIC, ...) becomes PNG here. A PNG/WebP that already fits is sent
 * untouched; a converted image has no EXIF/GPS data, and is scaled down until it fits.
 */
object ImageSender {
    /** The most we read from another app's stream. */
    private const val MAX_RAW_BYTES = 32L * 1024 * 1024
    /** What the engine accepts (see klip-android: max_clip_bytes). */
    private const val MAX_SEND_BYTES = 12_000_000
    private const val MAX_PIXELS = 36_000_000L
    /** While decoding, the longest side is capped here so a 50 MP photo cannot run the app out of memory. */
    private const val DECODE_MAX_SIDE = 4096

    /** Read the whole stream, refusing anything bigger than [MAX_RAW_BYTES]. */
    fun readBounded(ctx: Context, uri: Uri): ByteArray {
        val input = ctx.contentResolver.openInputStream(uri) ?: throw IOException("cannot open the image")
        input.use { ins ->
            val out = ByteArrayOutputStream()
            val buf = ByteArray(64 * 1024)
            var total = 0L
            while (true) {
                val n = ins.read(buf)
                if (n < 0) break
                total += n
                if (total > MAX_RAW_BYTES) throw IOException("the image is larger than 32 MB")
                out.write(buf, 0, n)
            }
            return out.toByteArray()
        }
    }

    /** Bytes for the engine's sendImage: PNG or WebP, within the size and pixel limits. */
    fun prepare(raw: ByteArray): ByteArray {
        if (raw.isEmpty()) throw IOException("the image is empty")
        val bounds = bounds(raw)
        val pixels = bounds.outWidth.toLong() * bounds.outHeight.toLong()
        if (bounds.outWidth <= 0 || bounds.outHeight <= 0) throw IOException("this is not an image Klip can read")
        if (isPngOrWebp(raw) && raw.size <= MAX_SEND_BYTES && pixels <= MAX_PIXELS) return raw

        var bmp = decode(raw, bounds) ?: throw IOException("this is not an image Klip can read")
        try {
            repeat(7) {
                val out = ByteArrayOutputStream()
                bmp.compress(Bitmap.CompressFormat.PNG, 100, out)
                if (out.size() <= MAX_SEND_BYTES) return out.toByteArray()
                val next = Bitmap.createScaledBitmap(
                    bmp, max(1, (bmp.width * 0.75f).toInt()), max(1, (bmp.height * 0.75f).toInt()), true
                )
                if (next !== bmp) bmp.recycle()
                bmp = next
            }
        } finally {
            bmp.recycle()
        }
        throw IOException("the image is too large to send")
    }

    private fun bounds(raw: ByteArray): BitmapFactory.Options =
        BitmapFactory.Options().also {
            it.inJustDecodeBounds = true
            BitmapFactory.decodeByteArray(raw, 0, raw.size, it)
        }

    private fun decode(raw: ByteArray, b: BitmapFactory.Options): Bitmap? {
        var sample = 1
        while (max(b.outWidth, b.outHeight) / sample > DECODE_MAX_SIDE) sample *= 2
        val bmp = BitmapFactory.decodeByteArray(raw, 0, raw.size, BitmapFactory.Options().also { it.inSampleSize = sample })
            ?: return null
        val degrees = runCatching {
            when (ExifInterface(ByteArrayInputStream(raw)).getAttributeInt(ExifInterface.TAG_ORIENTATION, 1)) {
                ExifInterface.ORIENTATION_ROTATE_90 -> 90f
                ExifInterface.ORIENTATION_ROTATE_180 -> 180f
                ExifInterface.ORIENTATION_ROTATE_270 -> 270f
                else -> 0f
            }
        }.getOrDefault(0f)
        if (degrees == 0f) return bmp
        val rotated = Bitmap.createBitmap(bmp, 0, 0, bmp.width, bmp.height, Matrix().apply { postRotate(degrees) }, true)
        if (rotated !== bmp) bmp.recycle()
        return rotated
    }

    private fun isPngOrWebp(b: ByteArray): Boolean {
        val png = byteArrayOf(0x89.toByte(), 'P'.code.toByte(), 'N'.code.toByte(), 'G'.code.toByte(), 13, 10, 26, 10)
        if (b.size >= 8 && png.indices.all { b[it] == png[it] }) return true
        return b.size >= 12 && String(b, 0, 4, Charsets.ISO_8859_1) == "RIFF" &&
            String(b, 8, 4, Charsets.ISO_8859_1) == "WEBP"
    }
}
