package com.klip.app

/** What to do to a decoded picture so it looks the way its EXIF orientation says. Pure, unit-tested. */
object ExifOrient {
    /** Rotate clockwise by [rotateDegrees], then flip horizontally and/or vertically. */
    data class Ops(val rotateDegrees: Float, val flipHorizontal: Boolean, val flipVertical: Boolean)

    fun ops(orientation: Int): Ops = when (orientation) {
        2 -> Ops(0f, true, false)
        3 -> Ops(180f, false, false)
        4 -> Ops(0f, false, true)
        5 -> Ops(90f, true, false) // transpose
        6 -> Ops(90f, false, false)
        7 -> Ops(270f, true, false) // transverse
        8 -> Ops(270f, false, false)
        else -> Ops(0f, false, false)
    }
}
