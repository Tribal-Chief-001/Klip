package com.klip.app.ui

import android.app.Activity
import android.app.Dialog
import android.content.Context
import android.content.res.ColorStateList
import android.content.res.Configuration
import android.graphics.Color
import android.graphics.Typeface
import android.graphics.drawable.Drawable
import android.graphics.drawable.GradientDrawable
import android.graphics.drawable.RippleDrawable
import android.os.Build
import android.text.TextUtils
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.view.WindowManager
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import androidx.core.view.WindowCompat
import com.klip.app.R

/**
 * Klip's look in one place. Two themes (follows the system), one signal colour, hairline borders,
 * no shadows. Everything is drawn with framework widgets so the APK stays small.
 */
class Pal(val dark: Boolean) {
    val bg = if (dark) 0xFF0B0E10.toInt() else 0xFFF3F4F0.toInt()
    val surface = if (dark) 0xFF12171A.toInt() else 0xFFFFFFFF.toInt()
    val surface2 = if (dark) 0xFF1A2126.toInt() else 0xFFEAEDE6.toInt()
    val line = if (dark) 0xFF252D33.toInt() else 0xFFDADED4.toInt()
    val text = if (dark) 0xFFE8EDF0.toInt() else 0xFF12161A.toInt()
    val dim = if (dark) 0xFF8C979F.toInt() else 0xFF5A646B.toInt()
    val faint = if (dark) 0xFF56616A.toInt() else 0xFF9AA3A9.toInt()

    /** The one bright colour: live connection, "done". */
    val signal = if (dark) 0xFFC8F25A.toInt() else 0xFF4D7A00.toInt()

    /** Filled primary button. */
    val action = if (dark) 0xFFC8F25A.toInt() else 0xFF12161A.toInt()
    val onAction = if (dark) 0xFF0B0E10.toInt() else 0xFFF3F4F0.toInt()
    val warn = if (dark) 0xFFFFB84D.toInt() else 0xFFA85F00.toInt()
    val bad = if (dark) 0xFFFF7A6B.toInt() else 0xFFC4372A.toInt()

    companion object {
        fun of(c: Context) = Pal(
            c.resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK ==
                Configuration.UI_MODE_NIGHT_YES
        )
    }
}

fun Context.dp(v: Int): Int = (v * resources.displayMetrics.density + 0.5f).toInt()
fun Context.dpf(v: Float): Float = v * resources.displayMetrics.density
fun withAlpha(color: Int, alpha: Float): Int =
    Color.argb((alpha * 255).toInt(), Color.red(color), Color.green(color), Color.blue(color))

fun rounded(color: Int, radiusPx: Float, stroke: Int? = null, strokePx: Int = 1): GradientDrawable =
    GradientDrawable().apply {
        setColor(color)
        cornerRadius = radiusPx
        if (stroke != null) setStroke(strokePx, stroke)
    }

/** Press feedback on top of [base]. */
fun View.rippleOver(pal: Pal, base: Drawable?, radiusPx: Float) {
    val mask = rounded(Color.WHITE, radiusPx)
    background = RippleDrawable(ColorStateList.valueOf(withAlpha(pal.text, 0.12f)), base, mask)
}

object Type {
    fun medium(): Typeface =
        if (Build.VERSION.SDK_INT >= 28) Typeface.create(Typeface.DEFAULT, 500, false)
        else Typeface.create("sans-serif-medium", Typeface.NORMAL)

    fun semibold(): Typeface =
        if (Build.VERSION.SDK_INT >= 28) Typeface.create(Typeface.DEFAULT, 650, false)
        else Typeface.create("sans-serif-medium", Typeface.BOLD)
}

fun Context.tv(
    text: CharSequence,
    sizeSp: Float = 15f,
    color: Int,
    weight: Int = 0, // 0 regular, 1 medium, 2 semibold
    mono: Boolean = false,
    lines: Int = 0
): TextView = TextView(this).apply {
    this.text = text
    textSize = sizeSp
    setTextColor(color)
    includeFontPadding = false
    setLineSpacing(0f, 1.18f)
    typeface = when {
        mono -> Typeface.MONOSPACE
        weight == 1 -> Type.medium()
        weight == 2 -> Type.semibold()
        else -> Typeface.DEFAULT
    }
    if (lines > 0) {
        maxLines = lines
        ellipsize = TextUtils.TruncateAt.END
    }
}

fun lp(w: Int = ViewGroup.LayoutParams.MATCH_PARENT, h: Int = ViewGroup.LayoutParams.WRAP_CONTENT) =
    LinearLayout.LayoutParams(w, h)

fun LinearLayout.add(v: View, top: Int = 0, bottom: Int = 0, ctx: Context = context): View {
    addView(v, lp().apply { topMargin = ctx.dp(top); bottomMargin = ctx.dp(bottom) })
    return v
}

fun column(ctx: Context) = LinearLayout(ctx).apply { orientation = LinearLayout.VERTICAL }

/** A bordered surface: the only container style the app uses. */
fun card(ctx: Context, pal: Pal, padDp: Int = 16): LinearLayout = column(ctx).apply {
    background = rounded(pal.surface, ctx.dpf(18f), pal.line, 1)
    setPadding(ctx.dp(padDp), ctx.dp(padDp), ctx.dp(padDp), ctx.dp(padDp))
}

fun primaryButton(ctx: Context, pal: Pal, label: String, onClick: () -> Unit): TextView =
    ctx.tv(label, 16f, pal.onAction, 2).apply {
        gravity = Gravity.CENTER
        minHeight = ctx.dp(54)
        setPadding(ctx.dp(20), 0, ctx.dp(20), 0)
        rippleOver(pal, rounded(pal.action, ctx.dpf(16f)), ctx.dpf(16f))
        isClickable = true
        setOnClickListener { onClick() }
    }

fun secondaryButton(ctx: Context, pal: Pal, label: String, onClick: () -> Unit): TextView =
    ctx.tv(label, 15f, pal.text, 1).apply {
        gravity = Gravity.CENTER
        minHeight = ctx.dp(50)
        setPadding(ctx.dp(18), 0, ctx.dp(18), 0)
        rippleOver(pal, rounded(pal.surface2, ctx.dpf(16f), pal.line, 1), ctx.dpf(16f))
        isClickable = true
        setOnClickListener { onClick() }
    }

fun textButton(ctx: Context, pal: Pal, label: String, color: Int = pal.signal, onClick: () -> Unit): TextView =
    ctx.tv(label, 14f, color, 1).apply {
        gravity = Gravity.CENTER_VERTICAL
        minHeight = ctx.dp(44)
        setPadding(ctx.dp(4), 0, ctx.dp(4), 0)
        isClickable = true
        setOnClickListener { onClick() }
    }

/** Small caps-like heading above a group. */
fun sectionLabel(ctx: Context, pal: Pal, text: String): TextView =
    ctx.tv(text.uppercase(), 11.5f, pal.dim, 1).apply { letterSpacing = 0.1f }

fun chip(ctx: Context, text: String, color: Int): TextView =
    ctx.tv(text, 11.5f, color, 1).apply {
        gravity = Gravity.CENTER
        setPadding(ctx.dp(9), ctx.dp(4), ctx.dp(9), ctx.dp(4))
        background = rounded(withAlpha(color, 0.14f), ctx.dpf(20f))
    }

fun dot(ctx: Context, color: Int, sizeDp: Int = 9, hollow: Boolean = false): View = View(ctx).apply {
    layoutParams = LinearLayout.LayoutParams(ctx.dp(sizeDp), ctx.dp(sizeDp))
    background = GradientDrawable().apply {
        shape = GradientDrawable.OVAL
        if (hollow) setStroke(ctx.dp(2), color) else setColor(color)
    }
}

fun divider(ctx: Context, pal: Pal): View = View(ctx).apply {
    layoutParams = LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 1)
    setBackgroundColor(pal.line)
}

/** Edge-to-edge window with system-bar padding applied to [root]'s content. */
fun Activity.edgeToEdge(
    pal: Pal, root: View, extraTopDp: Int = 0, extraBottomDp: Int = 0, onTopInset: ((Int) -> Unit)? = null
) {
    WindowCompat.setDecorFitsSystemWindows(window, false)
    window.statusBarColor = Color.TRANSPARENT
    window.navigationBarColor = Color.TRANSPARENT
    WindowCompat.getInsetsController(window, window.decorView).apply {
        isAppearanceLightStatusBars = !pal.dark
        isAppearanceLightNavigationBars = !pal.dark
    }
    val l = root.paddingLeft
    val t = root.paddingTop
    val r = root.paddingRight
    val b = root.paddingBottom
    root.setOnApplyWindowInsetsListener { v, insets ->
        val bars = androidx.core.view.WindowInsetsCompat.toWindowInsetsCompat(insets, v)
            .getInsets(androidx.core.view.WindowInsetsCompat.Type.systemBars() or
                androidx.core.view.WindowInsetsCompat.Type.displayCutout())
        v.setPadding(l + bars.left, t + bars.top + dp(extraTopDp), r + bars.right, b + bars.bottom + dp(extraBottomDp))
        onTopInset?.invoke(bars.top)
        insets
    }
    root.requestApplyInsets()
}

/** A page: scrolling column with standard gutters and a solid strip behind the status bar. */
class Page(val activity: Activity, val pal: Pal) {
    val scroll = ScrollView(activity).apply {
        setBackgroundColor(pal.bg)
        isFillViewport = true
        overScrollMode = View.OVER_SCROLL_NEVER
    }
    val body = column(activity).apply {
        setPadding(activity.dp(20), activity.dp(8), activity.dp(20), activity.dp(32))
    }

    init {
        scroll.addView(body, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT))
        // Content scrolls under the transparent status bar; this strip keeps the clock readable.
        val scrim = View(activity).apply { setBackgroundColor(pal.bg) }
        val root = FrameLayout(activity)
        root.addView(scroll, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
        root.addView(scrim, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, Gravity.TOP))
        activity.edgeToEdge(pal, body, extraTopDp = 6) { top ->
            scrim.layoutParams = (scrim.layoutParams as FrameLayout.LayoutParams).also { it.height = top }
        }
        activity.setContentView(root)
    }
}

/** Bottom sheet built from framework Dialog (no Material). */
class Sheet(private val activity: Activity, val pal: Pal) {
    private val dialog = Dialog(activity, R.style.Theme_Klip_Sheet)
    val body = column(activity).apply {
        background = rounded(pal.surface, 0f).apply {
            val r = activity.dpf(26f)
            cornerRadii = floatArrayOf(r, r, r, r, 0f, 0f, 0f, 0f)
        }
        setPadding(activity.dp(22), activity.dp(10), activity.dp(22), activity.dp(26))
    }

    init {
        // Grab handle.
        body.add(View(activity).apply {
            background = rounded(pal.line, activity.dpf(3f))
            layoutParams = LinearLayout.LayoutParams(activity.dp(40), activity.dp(5)).apply {
                gravity = Gravity.CENTER_HORIZONTAL
                bottomMargin = activity.dp(16)
            }
        })
        val scroll = ScrollView(activity).apply { addView(body) }
        dialog.setContentView(scroll)
        dialog.window?.apply {
            setLayout(WindowManager.LayoutParams.MATCH_PARENT, WindowManager.LayoutParams.WRAP_CONTENT)
            setGravity(Gravity.BOTTOM)
            setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE)
            navigationBarColor = pal.surface
        }
    }

    fun title(text: String, sub: String? = null) {
        body.add(activity.tv(text, 22f, pal.text, 2))
        if (sub != null) body.add(activity.tv(sub, 14.5f, pal.dim), top = 6)
        body.add(View(activity), bottom = 14)
    }

    fun onDismiss(f: () -> Unit) = dialog.setOnDismissListener { f() }
    fun cancelable(v: Boolean) = dialog.setCancelable(v)
    fun show(): Sheet { dialog.show(); return this }
    fun dismiss() { if (dialog.isShowing) dialog.dismiss() }
    val isShowing get() = dialog.isShowing
}
