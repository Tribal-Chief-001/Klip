package com.klip.app

/** Plain text for file transfers: sizes, notification lines, names. Pure, so it is unit-tested on the JVM. */
object FileText {
    /** One transfer as the progress line sees it. */
    data class Row(val outgoing: Boolean, val size: Long, val done: Long, val running: Boolean)

    fun size(bytes: Long): String {
        val kib = 1024.0
        val b = bytes.toDouble()
        return when {
            b >= kib * kib * kib -> "%.1f GiB".format(java.util.Locale.US, b / (kib * kib * kib))
            b >= kib * kib -> "%.1f MiB".format(java.util.Locale.US, b / (kib * kib))
            b >= kib -> "%.1f KiB".format(java.util.Locale.US, b / kib)
            else -> "$bytes B"
        }
    }

    /** Never the file's name: notifications can show on a locked screen. */
    fun received(from: String, bytes: Long) = "Received 1 file from $from (${size(bytes)})"

    /** "Sending 2 files, 45%"; empty when nothing is running. */
    fun progress(rows: List<Row>): String {
        val running = rows.filter { it.running }
        if (running.isEmpty()) return ""
        fun part(verb: String, n: Int) = "$verb $n file" + if (n == 1) "" else "s"
        val out = running.count { it.outgoing }
        val inc = running.size - out
        val total = running.sumOf { it.size }
        val pct = if (total == 0L) 100 else (running.sumOf { it.done } * 100 / total).toInt()
        val what = listOfNotNull(
            if (out > 0) part("Sending", out) else null,
            if (inc > 0) part(if (out > 0) "receiving" else "Receiving", inc) else null,
        ).joinToString(", ")
        return "$what, $pct%"
    }

    private val mimes = mapOf(
        "jpg" to "image/jpeg", "jpeg" to "image/jpeg", "png" to "image/png", "gif" to "image/gif",
        "webp" to "image/webp", "heic" to "image/heic", "pdf" to "application/pdf",
        "txt" to "text/plain", "csv" to "text/csv", "html" to "text/html", "json" to "application/json",
        "zip" to "application/zip", "apk" to "application/vnd.android.package-archive",
        "mp3" to "audio/mpeg", "m4a" to "audio/mp4", "wav" to "audio/wav", "ogg" to "audio/ogg",
        "mp4" to "video/mp4", "mkv" to "video/x-matroska", "webm" to "video/webm", "mov" to "video/quicktime",
        "doc" to "application/msword",
        "docx" to "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" to "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" to "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    )

    fun mimeFor(name: String): String =
        mimes[name.substringAfterLast('.', "").lowercase()] ?: "application/octet-stream"

    private const val MAX_NAME = 255

    /** A display name from another app, made safe to use as a file name: no path, no control characters. */
    fun safeName(raw: String?): String {
        val last = (raw ?: "").split('/', '\\').lastOrNull { it.isNotBlank() } ?: ""
        var name = last.filter { !it.isISOControl() }.trim().trimStart('.')
        if (name.length > MAX_NAME) {
            name = name.take(MAX_NAME)
            if (name.last().isHighSurrogate()) name = name.dropLast(1)
        }
        return name.ifBlank { "file" }
    }

    /** A folder path from the engine (`Top/sub`), cleaned component by component; never absolute, never `..`. */
    fun safeRelDir(raw: String): String =
        raw.split('/', '\\')
            .filter { it.isNotBlank() && it != "." && it != ".." }
            .take(16)
            .joinToString("/") { safeName(it) }
}
