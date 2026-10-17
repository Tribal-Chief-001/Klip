package com.klip.app

import org.junit.Assert.assertEquals
import org.junit.Test

class ExifOrientTest {
    private fun ops(o: Int) = ExifOrient.ops(o)

    @Test fun the_four_plain_rotations() {
        assertEquals(ExifOrient.Ops(0f, false, false), ops(1))
        assertEquals(ExifOrient.Ops(180f, false, false), ops(3))
        assertEquals(ExifOrient.Ops(90f, false, false), ops(6))
        assertEquals(ExifOrient.Ops(270f, false, false), ops(8))
    }

    @Test fun the_four_mirrored_ones_flip_as_well() {
        assertEquals(ExifOrient.Ops(0f, true, false), ops(2))
        assertEquals(ExifOrient.Ops(0f, false, true), ops(4))
        assertEquals(ExifOrient.Ops(90f, true, false), ops(5))
        assertEquals(ExifOrient.Ops(270f, true, false), ops(7))
    }

    @Test fun unknown_values_leave_the_picture_alone() {
        assertEquals(ExifOrient.Ops(0f, false, false), ops(0))
        assertEquals(ExifOrient.Ops(0f, false, false), ops(9))
        assertEquals(ExifOrient.Ops(0f, false, false), ops(-5))
    }

    @Test fun transpose_maps_the_top_left_pixel_to_itself_and_transverse_to_the_opposite_corner() {
        // A 4x2 picture; (x, y) -> where it lands after rotating clockwise by `deg` and then flipping horizontally.
        fun land(x: Int, y: Int, w: Int, h: Int, o: ExifOrient.Ops): Pair<Int, Int> {
            var (px, py, pw, ph) = listOf(x, y, w, h)
            repeat((o.rotateDegrees / 90f).toInt()) { // one clockwise quarter turn: (x, y) -> (h-1-y, x), size swaps
                val nx = ph - 1 - py
                val ny = px
                px = nx; py = ny
                val t = pw; pw = ph; ph = t
            }
            if (o.flipHorizontal) px = pw - 1 - px
            if (o.flipVertical) py = ph - 1 - py
            return px to py
        }
        assertEquals(0 to 0, land(0, 0, 4, 2, ops(5))) // transpose: (x, y) -> (y, x)
        assertEquals(1 to 3, land(3, 1, 4, 2, ops(5)))
        assertEquals(1 to 3, land(0, 0, 4, 2, ops(7))) // transverse: top-left goes to the bottom-right corner
    }
}
