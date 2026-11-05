package com.allternit.phonelink.core

import org.json.JSONArray
import org.json.JSONObject

/** Rendering, filtering and matching over a flattened [UiNode] list. Pure, unit-tested. */
object UiTree {

    /**
     * One line per node, in the iCTRL text-source shape:
     * `Button, label: 'Send', value: '…', id: 'com.x:id/send', {{x, y}, {w, h}}, clickable`
     * Android extras after the frame: text/desc when they differ from the
     * label, and state flags.
     */
    fun describe(n: UiNode): String {
        val parts = ArrayList<String>()
        parts.add(if (n.windowType != null) "Window(${n.windowType})" else n.shortClass)
        if (n.label.isNotEmpty()) parts.add("label: '${q(n.label)}'")
        n.value?.let { parts.add("value: '${q(it)}'") }
        n.resourceId?.takeIf { it.isNotEmpty() }?.let { parts.add("id: '${q(it)}'") }
        val b = n.bounds
        parts.add("{{${b.left}, ${b.top}}, {${b.width}, ${b.height}}}")
        if (n.editable && n.hint != null && n.text.isNullOrEmpty()) parts.add("hint: '${q(n.hint)}'")
        if (n.windowType != null && n.packageName != null) parts.add("package: '${n.packageName}'")
        val flags = ArrayList<String>()
        if (n.clickable) flags.add("clickable")
        if (n.longClickable) flags.add("long-clickable")
        if (n.editable) flags.add("editable")
        if (n.focused) flags.add("focused")
        if (n.scrollable) flags.add("scrollable")
        if (n.checkable) flags.add(if (n.checked) "checked" else "unchecked")
        if (n.selected) flags.add("selected")
        if (!n.enabled) flags.add("disabled")
        if (n.password) flags.add("password")
        if (!n.visibleToUser) flags.add("offscreen")
        if (flags.isNotEmpty()) parts.add(flags.joinToString(" "))
        return parts.joinToString(", ")
    }

    private fun q(s: String) = s.replace("\n", "\\n").replace("'", "\\'")

    fun nodeJson(n: UiNode, ref: String? = null): JSONObject {
        val o = JSONObject()
            .put("depth", n.depth)
            .put("class", n.className)
            .put("type", elementType(n))
            .put("label", n.label)
            .put("text", n.text ?: JSONObject.NULL)
            .put("content_desc", n.contentDesc ?: JSONObject.NULL)
            .put("resource_id", n.resourceId ?: JSONObject.NULL)
            .put("package", n.packageName ?: JSONObject.NULL)
            .put("bounds", JSONObject().put("x", n.bounds.left).put("y", n.bounds.top)
                .put("width", n.bounds.width).put("height", n.bounds.height))
            .put("clickable", n.clickable).put("editable", n.editable).put("focused", n.focused)
            .put("scrollable", n.scrollable).put("enabled", n.enabled).put("visible", n.visibleToUser)
        if (n.checkable) o.put("checked", n.checked)
        if (n.hint != null) o.put("hint", n.hint)
        if (n.windowType != null) o.put("window_type", n.windowType)
        if (ref != null) o.put("ref", ref)
        return o
    }

    /** iCTRL element_type vocabulary mapped onto Android classes. */
    fun elementType(n: UiNode): String {
        val c = n.className
        return when {
            n.windowType != null -> "window"
            n.editable || c.endsWith("EditText") || c.endsWith("AutoCompleteTextView") -> "textfield"
            c.endsWith("Switch") || c.endsWith("SwitchCompat") || c.endsWith("ToggleButton") || (n.checkable && c.endsWith("CheckBox")) -> "switch"
            c.endsWith("ImageButton") || c.endsWith("Button") -> "button"
            c.endsWith("ImageView") -> if (n.clickable) "button" else "image"
            c.endsWith("TextView") -> if (n.clickable) "button" else "label"
            c.contains("RecyclerView") || c.endsWith("ListView") || c.endsWith("ScrollView") || c.endsWith("GridView") -> "list"
            n.clickable -> "cell"
            else -> "other"
        }
    }

    fun typeMatches(n: UiNode, wanted: String?): Boolean {
        if (wanted == null || wanted.isEmpty() || wanted == "any") return true
        val t = elementType(n)
        return when (wanted.lowercase()) {
            "button" -> t == "button" || (n.clickable && t != "textfield")
            "textfield", "searchfield", "textview", "input" -> t == "textfield"
            "label", "text", "statictext" -> t == "label" || t == "button"
            "image", "icon" -> t == "image" || n.className.endsWith("ImageView") || n.className.endsWith("ImageButton")
            "cell" -> n.clickable
            "switch", "toggle", "checkbox" -> t == "switch" || n.checkable
            else -> t == wanted.lowercase() || n.shortClass.equals(wanted, ignoreCase = true)
        }
    }

    data class Filter(val label: String? = null, val type: String? = null, val text: String? = null) {
        val active: Boolean get() = !label.isNullOrEmpty() || !type.isNullOrEmpty() || !text.isNullOrEmpty()

        fun matches(n: UiNode): Boolean {
            if (!label.isNullOrEmpty()) {
                val l = label.lowercase()
                if (!n.label.lowercase().contains(l) && n.value?.lowercase()?.contains(l) != true) return false
            }
            if (!type.isNullOrEmpty() && !typeMatches(n, type)) return false
            if (!text.isNullOrEmpty()) {
                val t = text.lowercase()
                val hay = listOfNotNull(n.label, n.value, n.text, n.contentDesc, n.resourceId).joinToString("\u0000").lowercase()
                if (!hay.contains(t)) return false
            }
            return true
        }

        companion object {
            fun from(o: JSONObject?): Filter =
                if (o == null) Filter() else Filter(o.optStringOrNull("label"), o.optStringOrNull("type"), o.optStringOrNull("text"))
        }
    }

    class Selection(val nodes: List<UiNode>, val truncated: Boolean)

    /**
     * Applies max_depth, visible_only (node intersects the screen, plus its
     * ancestors), filter (matches plus ancestors) and max_elements.
     */
    fun select(
        all: List<UiNode>,
        screen: Bounds,
        filter: Filter = Filter(),
        maxDepth: Int = Int.MAX_VALUE,
        visibleOnly: Boolean = false,
        maxElements: Int = 2000,
    ): Selection {
        val keep = BooleanArray(all.size)
        val stack = ArrayList<Int>() // indices of the current ancestor chain
        val needsAncestors = filter.active || visibleOnly
        for (i in all.indices) {
            val n = all[i]
            while (stack.isNotEmpty() && all[stack.last()].depth >= n.depth) stack.removeAt(stack.size - 1)
            var ok = n.depth <= maxDepth
            if (ok && visibleOnly) ok = n.windowType != null || (n.visibleToUser && n.bounds.intersects(screen))
            if (ok && filter.active) ok = filter.matches(n)
            if (ok) {
                keep[i] = true
                if (needsAncestors) for (a in stack) keep[a] = true
            }
            stack.add(i)
        }
        val out = ArrayList<UiNode>()
        var truncated = false
        for (i in all.indices) {
            if (!keep[i]) continue
            if (out.size >= maxElements) { truncated = true; break }
            out.add(all[i])
        }
        return Selection(out, truncated)
    }

    fun render(nodes: List<UiNode>): String = nodes.joinToString("\n") { "  ".repeat(it.depth) + describe(it) }

    fun structured(nodes: List<UiNode>): JSONArray = JSONArray().also { a -> nodes.forEach { a.put(nodeJson(it)) } }

    // ── Matching (find_element / tap_element / find_and_tap) ───────────────

    class Match(val node: UiNode, val how: String)

    /**
     * iCTRL order: exact label → identifier → (match:'contains') substring.
     * Among equals, an element in view beats an offscreen one.
     */
    fun find(all: List<UiNode>, elementId: String, contains: Boolean, type: String?, screen: Bounds): List<Match> {
        val candidates = all.filter { it.windowType == null && typeMatches(it, type) }
        fun inView(n: UiNode) = n.visibleToUser && !n.bounds.isEmpty && n.bounds.intersects(screen)
        fun ordered(list: List<UiNode>, how: String) =
            list.sortedWith(compareByDescending<UiNode> { inView(it) }.thenByDescending { it.clickable })
                .map { Match(it, how) }

        val exactLabel = candidates.filter { it.label == elementId || it.text == elementId || it.contentDesc == elementId }
        if (exactLabel.isNotEmpty()) return ordered(exactLabel, "label")
        val byId = candidates.filter { it.resourceId == elementId || (it.shortId != null && it.shortId == elementId) }
        if (byId.isNotEmpty()) return ordered(byId, "id")
        if (!contains) return emptyList()
        val needle = elementId.lowercase()
        val sub = candidates.filter { n ->
            listOfNotNull(n.label, n.text, n.contentDesc, n.resourceId).any { it.lowercase().contains(needle) }
        }
        return ordered(sub, "contains")
    }

    fun visibility(n: UiNode, screen: Bounds): String = when {
        !n.visibleToUser || n.bounds.isEmpty || !n.bounds.intersects(screen) -> "offscreen"
        screen.contains(n.bounds) -> "in_view"
        else -> "partial"
    }

    /** The largest scrollable node on screen (target for coarse scroll). */
    fun mainScrollable(all: List<UiNode>, screen: Bounds): UiNode? =
        all.filter { it.scrollable && it.visibleToUser && it.bounds.intersects(screen) }
            .maxByOrNull { it.bounds.width.toLong() * it.bounds.height }
}

fun JSONObject.optStringOrNull(key: String): String? =
    if (!has(key) || isNull(key)) null else (opt(key) as? String ?: opt(key)?.toString())

fun JSONObject.optDoubleOrNull(key: String): Double? =
    if (!has(key) || isNull(key)) null else (opt(key) as? Number)?.toDouble() ?: (opt(key) as? String)?.toDoubleOrNull()

fun JSONObject.optBoolOrNull(key: String): Boolean? =
    if (!has(key) || isNull(key)) null else opt(key) as? Boolean
