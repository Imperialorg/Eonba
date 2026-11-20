package com.eon.assistant

import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Region
import android.util.AttributeSet
import android.view.View
import android.view.WindowInsets
import android.widget.Button
import android.widget.FrameLayout
import android.widget.ImageButton
import android.widget.ScrollView
import android.widget.TextView
import kotlin.math.roundToInt

internal class VoiceHudView(
    context: Context,
    toggleDetails: () -> Unit,
    checkProfile: () -> Unit,
    openControls: () -> Unit,
    dismiss: () -> Unit,
) : FrameLayout(context) {
    init {
        inflate(context, R.layout.voice_hud, this)
        setOnApplyWindowInsetsListener { _, insets ->
            val safe = insets.getInsets(WindowInsets.Type.systemBars() or
                WindowInsets.Type.displayCutout() or WindowInsets.Type.ime())
            setPadding(safe.left, safe.top, safe.right, safe.bottom)
            insets
        }
    }

    private val panel: ScrollView = findViewById(R.id.hud_panel)
    private val track: VoiceHudTrack = findViewById(R.id.hud_track)
    private val status: TextView = findViewById(R.id.hud_status)
    private val details: View = findViewById(R.id.hud_details)
    private val result: TextView = findViewById(R.id.hud_result)
    private val toggle: ImageButton = findViewById(R.id.hud_toggle)
    private val check: Button = findViewById(R.id.hud_check)
    private val position = IntArray(2)

    init {
        toggle.setOnClickListener { toggleDetails() }
        check.setOnClickListener { checkProfile() }
        findViewById<Button>(R.id.hud_controls).setOnClickListener { openControls() }
        findViewById<ImageButton>(R.id.hud_end).setOnClickListener { dismiss() }
    }

    override fun onMeasure(widthMeasureSpec: Int, heightMeasureSpec: Int) {
        val lp = panel.layoutParams as LayoutParams
        val available = MeasureSpec.getSize(widthMeasureSpec) - paddingLeft - paddingRight - lp.leftMargin - lp.rightMargin
        lp.width = minOf((360 * resources.displayMetrics.density).roundToInt(), available.coerceAtLeast(0))
        super.onMeasure(widthMeasureSpec, heightMeasureSpec)
    }

    fun render(frame: VoiceHudFrame?) {
        panel.visibility = if (frame == null) GONE else VISIBLE
        val expanded = frame?.expanded == true
        details.visibility = if (expanded) VISIBLE else GONE
        if (!expanded) panel.scrollTo(0, 0)
        toggle.rotation = if (expanded) 180f else 0f
        toggle.contentDescription = context.getString(if (expanded) R.string.voice_hud_collapse else R.string.voice_hud_expand)
        check.isEnabled = expanded && frame.check !is ProfileCheck.Requested
        track.check = frame?.check ?: ProfileCheck.Idle
        val recommendation = (frame?.check as? ProfileCheck.Result)?.recommendation
        result.visibility = if (recommendation == null) GONE else VISIBLE
        result.text = recommendation?.let { context.getString(R.string.voice_hud_recommendation, it) }
        status.text = when (frame?.check) {
            null -> null
            ProfileCheck.Idle -> context.getString(R.string.voice_hud_ready)
            is ProfileCheck.Requested -> context.getString(R.string.voice_hud_checking)
            is ProfileCheck.Result -> context.getString(
                if (recommendation == null) R.string.voice_hud_failed else R.string.voice_hud_checked
            )
        }
    }

    fun fillTouchableRegion(region: Region) {
        region.setEmpty()
        if (!panel.isShown || !panel.isLaidOut) return
        panel.getLocationInWindow(position)
        region.set(position[0], position[1], position[0] + panel.width, position[1] + panel.height)
    }
}

/** Discrete milestones (session, request, result), not a waveform or an estimate of progress. */
internal class VoiceHudTrack(context: Context, attrs: AttributeSet) : View(context, attrs) {
    private val pen = Paint(Paint.ANTI_ALIAS_FLAG)
    var check: ProfileCheck = ProfileCheck.Idle
        set(value) {
            if (value != field) {
                field = value
                invalidate()
            }
        }

    override fun onDraw(canvas: Canvas) {
        val phase = check
        val reached = when (phase) {
            ProfileCheck.Idle -> 1
            is ProfileCheck.Requested -> 2
            is ProfileCheck.Result -> 3
        }
        val y = height / 2f
        val radius = minOf(width / 12f, height / 4f)
        pen.strokeWidth = resources.displayMetrics.density * 1.5f
        for (i in 0..2) {
            val x = width * (2 * i + 1) / 6f
            pen.color = if (i < reached) 0xff11665e.toInt() else 0xff68787a.toInt()
            if (i != 0) canvas.drawLine(x - width / 3f + radius, y, x - radius, y, pen)
            if (i == 2 && phase is ProfileCheck.Result && phase.recommendation == null) {
                pen.color = 0xff983b24.toInt()
                canvas.drawLine(x - radius, y - radius, x + radius, y + radius, pen)
                canvas.drawLine(x - radius, y + radius, x + radius, y - radius, pen)
                continue
            }
            pen.style = if (i < reached) Paint.Style.FILL else Paint.Style.STROKE
            canvas.drawCircle(x, y, radius, pen)
        }
    }
}
