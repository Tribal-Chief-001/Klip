package com.klip.app.ui

import android.animation.ValueAnimator
import android.content.Context
import android.graphics.Canvas
import android.graphics.DashPathEffect
import android.graphics.Paint
import android.graphics.Path
import android.graphics.PathMeasure
import android.view.View
import android.view.animation.LinearInterpolator

/**
 * The home screen's signature element: this phone on the left, your devices on the right, joined by
 * a line. A bright pulse travels along every live link; offline links are dashed; with no devices a
 * dashed stub points at an empty slot. It animates only while it is on screen.
 */
class LinkView(context: Context, private val pal: Pal) : View(context) {

    class Node(val name: String, val online: Boolean)

    private var nodes: List<Node> = emptyList()
    private var phase = 0f
    private val animator = ValueAnimator.ofFloat(0f, 1f).apply {
        duration = 2600
        repeatCount = ValueAnimator.INFINITE
        interpolator = LinearInterpolator()
        addUpdateListener { phase = it.animatedValue as Float; invalidate() }
    }

    private val line = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
        strokeWidth = context.dpf(2f)
        strokeCap = Paint.Cap.ROUND
    }
    private val fill = Paint(Paint.ANTI_ALIAS_FLAG).apply { style = Paint.Style.FILL }
    private val ring = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
        strokeWidth = context.dpf(2f)
    }
    private val label = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        textSize = context.dpf(12.5f)
        textAlign = Paint.Align.CENTER
        typeface = Type.medium()
    }
    private val path = Path()
    private val measure = PathMeasure()
    private val pos = FloatArray(2)

    fun set(newNodes: List<Node>) {
        val changed = newNodes.size != nodes.size ||
            newNodes.indices.any { newNodes[it].name != nodes[it].name || newNodes[it].online != nodes[it].online }
        nodes = newNodes
        if (changed) {
            requestLayout()
            updateAnimation()
            invalidate()
        }
    }

    private fun updateAnimation() {
        val wanted = nodes.any { it.online } && isShown
        if (wanted && !animator.isStarted) animator.start()
        if (!wanted && animator.isStarted) animator.cancel()
    }

    override fun onAttachedToWindow() { super.onAttachedToWindow(); updateAnimation() }
    override fun onDetachedFromWindow() { animator.cancel(); super.onDetachedFromWindow() }
    override fun onVisibilityAggregated(isVisible: Boolean) { super.onVisibilityAggregated(isVisible); updateAnimation() }

    override fun onMeasure(w: Int, h: Int) {
        val rows = nodes.size.coerceIn(1, 3)
        val height = context.dp(78 + 52 * (rows - 1))
        setMeasuredDimension(MeasureSpec.getSize(w), resolveSize(height, h))
    }

    override fun onDraw(c: Canvas) {
        val r = context.dpf(8f)
        val left = context.dpf(46f)
        val right = width - context.dpf(46f)
        val cy = (height - context.dpf(22f)) / 2f

        // Phone node (always present).
        drawNode(c, left, cy, filled = true, color = pal.text, hollow = false)
        drawLabel(c, "This phone", left, cy + r + context.dpf(20f), pal.dim)

        val shown = nodes.take(3)
        if (shown.isEmpty()) {
            // Empty slot with a dashed stub: "a device goes here".
            line.color = pal.faint
            line.pathEffect = DashPathEffect(floatArrayOf(context.dpf(5f), context.dpf(7f)), 0f)
            c.drawLine(left + r + context.dpf(8f), cy, right - r - context.dpf(8f), cy, line)
            line.pathEffect = null
            ring.color = pal.faint
            ring.pathEffect = DashPathEffect(floatArrayOf(context.dpf(3f), context.dpf(4f)), 0f)
            c.drawCircle(right, cy, r, ring)
            ring.pathEffect = null
            drawLabel(c, "no device yet", right, cy + r + context.dpf(20f), pal.faint)
            return
        }

        val slot = context.dpf(52f)
        val top = cy - slot * (shown.size - 1) / 2f
        shown.forEachIndexed { i, n ->
            val ny = top + slot * i
            val color = if (n.online) pal.signal else pal.faint
            path.reset()
            path.moveTo(left + r + context.dpf(6f), cy)
            val mid = (left + right) / 2f
            path.cubicTo(mid, cy, mid, ny, right - r - context.dpf(6f), ny)

            line.color = if (n.online) withAlpha(pal.signal, 0.55f) else pal.faint
            line.pathEffect = if (n.online) null else DashPathEffect(floatArrayOf(context.dpf(5f), context.dpf(7f)), 0f)
            c.drawPath(path, line)
            line.pathEffect = null

            if (n.online) {
                measure.setPath(path, false)
                for (k in 0..1) {
                    val t = (phase + k * 0.5f) % 1f
                    measure.getPosTan(measure.length * t, pos, null)
                    val fade = 1f - kotlin.math.abs(t - 0.5f) * 0.9f
                    fill.color = withAlpha(pal.signal, fade.coerceIn(0.2f, 1f))
                    c.drawCircle(pos[0], pos[1], context.dpf(3.2f), fill)
                }
            }
            drawNode(c, right, ny, filled = n.online, color = color, hollow = !n.online)
            val name = if (i == 2 && nodes.size > 3) "+${nodes.size - 2} more" else n.name
            drawLabel(c, ellipsize(name), right, ny + r + context.dpf(18f), if (n.online) pal.text else pal.dim)
        }
    }

    private fun ellipsize(s: String) = if (s.length > 14) s.take(13) + "…" else s

    private fun drawNode(c: Canvas, x: Float, y: Float, filled: Boolean, color: Int, hollow: Boolean) {
        val r = context.dpf(8f)
        if (filled && !hollow) {
            // soft halo, then the node
            fill.color = withAlpha(color, 0.16f)
            c.drawCircle(x, y, r + context.dpf(6f), fill)
            fill.color = color
            c.drawCircle(x, y, r, fill)
        } else {
            ring.color = color
            c.drawCircle(x, y, r, ring)
        }
    }

    private fun drawLabel(c: Canvas, text: String, x: Float, y: Float, color: Int) {
        label.color = color
        c.drawText(text, x, y, label)
    }
}
