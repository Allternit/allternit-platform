package com.allternit.phonelink.core

import org.json.JSONObject
import java.util.Base64

/**
 * Pet offers on Android (Phone Link): notice an error dialog in another app and offer help.
 * Pure Kotlin, unit-tested on the JVM. The rules, copy, block defaults and setting defaults all
 * come from [PetOfferContract] (generated from contracts/pet-offers/pet-offers-v1.json).
 *
 * Privacy: the text read from a dialog stays in memory. It never goes in a log, a notification or a
 * URL query string; it leaves this file only inside the `#pet-offer=` fragment of [fixItUrl].
 */
object PetNotice {
    data class Trigger(val kind: PetOfferKind, val ruleId: String, val line: String)

    private val compiled: List<Pair<PetOfferRule, Regex>> = PetOfferContract.RULES.map { rule ->
        val options = HashSet<RegexOption>()
        if (rule.caseInsensitive) options.add(RegexOption.IGNORE_CASE)
        if (rule.multiline) options.add(RegexOption.MULTILINE)
        rule to Regex(rule.pattern, options)
    }

    /**
     * The contract's matcher.steps: normalize line breaks, keep the last [maxTextChars] characters,
     * walk the rules in file order (dialog-scope rules only for dialog text), first match wins.
     * The line is the whole line holding the start of the match, trimmed, cut to lineMaxChars.
     */
    fun match(text: String, dialog: Boolean, maxTextChars: Int = PetOfferContract.DEFAULT_MAX_TEXT_CHARS): Trigger? {
        var t = text.replace("\r\n", "\n").replace('\r', '\n')
        if (maxTextChars > 0 && t.length > maxTextChars) t = t.substring(t.length - maxTextChars)
        for ((rule, re) in compiled) {
            if (rule.scope == PetOfferRuleScope.DIALOG && !dialog) continue
            val m = re.find(t) ?: continue
            val at = m.range.first
            val start = if (at <= 0) 0 else t.lastIndexOf('\n', at - 1) + 1
            var end = t.indexOf('\n', at)
            if (end < 0) end = t.length
            val line = t.substring(start, end).trim().take(PetOfferContract.LINE_MAX_CHARS)
            return Trigger(rule.kind, rule.id, line)
        }
        return null
    }

    /** Text of one window for the matcher. Secure (password) and editable fields are never read. */
    fun textOf(nodes: List<UiNode>, maxTextChars: Int = PetOfferContract.DEFAULT_MAX_TEXT_CHARS): String {
        val sb = StringBuilder()
        for (n in nodes) {
            if (n.windowType != null) continue
            if (n.password || n.editable) continue
            val s = n.text?.takeIf { it.isNotBlank() } ?: n.contentDesc?.takeIf { it.isNotBlank() } ?: continue
            if (sb.isNotEmpty()) sb.append('\n')
            sb.append(s.trim())
        }
        val all = sb.toString()
        return if (maxTextChars > 0 && all.length > maxTextChars) all.substring(all.length - maxTextChars) else all
    }

    /** True when an Android window class is one of the contract's dialog classes. */
    fun isDialogClass(className: String?): Boolean =
        className != null && className in PetOfferContract.DIALOG_ANDROID_CLASS_NAMES

    private val systemDialogSubject = Regex("^(.+?)\\s+(keeps stopping|has stopped|isn['’]t responding)\\b", RegexOption.IGNORE_CASE)

    /**
     * Android's own crash and not-responding dialogs come from the system ("android" package), so the
     * app they are about is only in their text ("Chrome keeps stopping"). Returns that name, or null.
     */
    fun systemDialogApp(line: String): String? =
        systemDialogSubject.find(line.trim())?.groupValues?.get(1)?.trim()?.takeIf { it.isNotEmpty() && it.length <= 80 }

    /** Offer line shown in Phone Link (never in a notification): "{App} is showing an error. Fix it?" etc. */
    fun copy(kind: PetOfferKind, app: String?): String =
        PetOfferContract.COPY_BY_KIND.getValue(kind)
            .replace(PetOfferContract.APP_PLACEHOLDER, app?.takeIf { it.isNotBlank() } ?: PetOfferContract.APP_FALLBACK)

    /**
     * `https://m.allternit.com/#pet-offer=<payload>`. The payload is base64url (no padding) of the UTF-8
     * JSON `{app, appId, kind, line, text}`; kind is the contract wire value. A fragment never reaches the
     * server, so the screen text stays on the phone until the user sends it.
     */
    fun fixItUrl(app: String?, appId: String?, trigger: Trigger, text: String, base: String = "https://m.allternit.com/"): String =
        "$base#pet-offer=${encodePayload(app, appId, trigger, text)}"

    fun encodePayload(app: String?, appId: String?, trigger: Trigger, text: String): String {
        val o = JSONObject()
            .put("app", app?.takeIf { it.isNotBlank() } ?: PetOfferContract.APP_FALLBACK)
            .put("appId", appId ?: JSONObject.NULL)
            .put("kind", trigger.kind.wire)
            .put("line", trigger.line)
            .put("text", text)
        return Base64.getUrlEncoder().withoutPadding().encodeToString(o.toString().toByteArray(Charsets.UTF_8))
    }
}

/**
 * Device-local pet-offer settings (SharedPreferences through [KeyValueStore]) with the contract's
 * keys and defaults. Lists are stored one entry per line (app ids and app names never hold a newline).
 */
class PetOfferSettings(private val kv: KeyValueStore) {
    var enabled: Boolean
        get() = kv.getBoolean(PetOfferContract.KEY_ENABLED, PetOfferContract.DEFAULT_ENABLED)
        set(v) = kv.edit { putBoolean(PetOfferContract.KEY_ENABLED, v) }

    /** The one switch: read other apps' dialogs at all. Default from the contract (off). */
    var readOtherApps: Boolean
        get() = kv.getBoolean(PetOfferContract.KEY_READ_OTHER_APPS, PetOfferContract.READ_OTHER_APPS_DEFAULT)
        set(v) = kv.edit { putBoolean(PetOfferContract.KEY_READ_OTHER_APPS, v) }

    val cooldownMs: Long get() = kv.getLong(PetOfferContract.KEY_COOLDOWN_MS, PetOfferContract.DEFAULT_COOLDOWN_MS)
    val maxTextChars: Int get() = kv.getInt(PetOfferContract.KEY_MAX_TEXT_CHARS, PetOfferContract.DEFAULT_MAX_TEXT_CHARS)

    /** The service reads other apps' dialogs only when this is true. */
    val watching: Boolean get() = enabled && readOtherApps

    /** The settings switch "Offer help when an app shows an error". Turning it on also turns offers on. */
    fun setWatching(on: Boolean) = kv.edit {
        putBoolean(PetOfferContract.KEY_READ_OTHER_APPS, on)
        if (on) putBoolean(PetOfferContract.KEY_ENABLED, true)
    }

    val blockList: List<String> get() = list(PetOfferContract.KEY_BLOCK_LIST, PetOfferContract.DEFAULT_BLOCK_LIST)
    val muted: List<String> get() = list(PetOfferContract.KEY_MUTED, PetOfferContract.DEFAULT_MUTED)

    fun addBlock(entry: String) = add(PetOfferContract.KEY_BLOCK_LIST, blockList, entry)
    fun removeBlock(entry: String) = remove(PetOfferContract.KEY_BLOCK_LIST, blockList, entry)
    fun mute(entry: String) = add(PetOfferContract.KEY_MUTED, muted, entry)
    fun unmute(entry: String) = remove(PetOfferContract.KEY_MUTED, muted, entry)

    private fun list(key: String, default: List<String>): List<String> =
        kv.getString(key)?.split('\n')?.map { it.trim() }?.filter { it.isNotEmpty() } ?: default

    private fun clean(entry: String) = entry.replace('\n', ' ').replace('\r', ' ').trim()

    private fun add(key: String, current: List<String>, entry: String): Boolean {
        val e = clean(entry)
        if (e.isEmpty() || current.any { it.equals(e, ignoreCase = true) }) return false
        kv.edit { putString(key, (current + e).joinToString("\n")) }
        return true
    }

    private fun remove(key: String, current: List<String>, entry: String): Boolean {
        val e = clean(entry)
        val next = current.filterNot { it.equals(e, ignoreCase = true) }
        if (next.size == current.size) return false
        kv.edit { putString(key, next.joinToString("\n")) }
        return true
    }
}

/**
 * Decides whether a window may be read and whether an offer may be shown.
 * Order (decision 4): blocked or muted apps are never read at all; the check is by app NAME first,
 * then by package (some package ids in the defaults are best guesses).
 * Cooldown, the 30 s re-read skip and "never the same line twice" live in memory only.
 */
class PetNoticeGate(private val settings: PetOfferSettings, private val ownPackage: String? = null) {
    enum class Block { OWN_APP, PASSWORD_MANAGER, BANKING, MESSAGES, USER_BLOCKED, MUTED }

    private val bankingWords: List<Regex> = PetOfferContract.BANKING_NAME_KEYWORDS.map {
        Regex("\\b" + Regex.escape(it) + "\\b", RegexOption.IGNORE_CASE)
    }
    private val lastRead = HashMap<String, Long>()
    private val lastOffer = HashMap<String, Long>()
    private val linesSeen = HashMap<String, LinkedHashSet<String>>()

    /** Why this app must not be read, or null when it may be. Name checks run before package checks. */
    fun blockReason(appName: String?, appId: String?): Block? {
        val name = appName?.trim()?.takeIf { it.isNotEmpty() }
        val id = appId?.trim()?.takeIf { it.isNotEmpty() }
        val user = settings.blockList
        val muted = settings.muted
        if (name != null) {
            if (PetOfferContract.PASSWORD_MANAGER_NAMES.any { it.equals(name, ignoreCase = true) }) return Block.PASSWORD_MANAGER
            if (bankingWords.any { it.containsMatchIn(name) }) return Block.BANKING
            if (user.any { it.equals(name, ignoreCase = true) }) return Block.USER_BLOCKED
            if (muted.any { it.equals(name, ignoreCase = true) }) return Block.MUTED
        }
        if (id != null) {
            if (ownPackage != null && id == ownPackage) return Block.OWN_APP
            if (PetOfferContract.PASSWORD_MANAGER_APP_IDS.any { it.equals(id, ignoreCase = true) }) return Block.PASSWORD_MANAGER
            if (PetOfferContract.MESSAGES_ANDROID_PACKAGES.any { it.equals(id, ignoreCase = true) }) return Block.MESSAGES
            if (user.any { it.equals(id, ignoreCase = true) }) return Block.USER_BLOCKED
            if (muted.any { it.equals(id, ignoreCase = true) }) return Block.MUTED
        }
        return null
    }

    /** False when the same window was read under limits.sameWindowRereadMs ago. Records the read when true. */
    @Synchronized
    fun shouldRead(readKey: String, nowMs: Long): Boolean {
        val last = lastRead[readKey]
        if (last != null && nowMs - last < PetOfferContract.SAME_WINDOW_REREAD_MS) return false
        lastRead[readKey] = nowMs
        if (lastRead.size > 256) lastRead.entries.removeIf { nowMs - it.value >= PetOfferContract.SAME_WINDOW_REREAD_MS }
        return true
    }

    /**
     * One offer per [windowKey] per cooldown, and never the same matched line twice for one window.
     * Records the offer when it returns true.
     */
    @Synchronized
    fun allowOffer(windowKey: String, line: String, nowMs: Long): Boolean {
        val seen = linesSeen.getOrPut(windowKey) { LinkedHashSet() }
        if (line in seen) return false
        val last = lastOffer[windowKey]
        if (last != null && nowMs - last < settings.cooldownMs) return false
        lastOffer[windowKey] = nowMs
        if (lastOffer.size > 256) lastOffer.entries.removeIf { nowMs - it.value >= settings.cooldownMs }
        seen.add(line)
        if (seen.size > 32) seen.remove(seen.first())
        if (linesSeen.size > 128) linesSeen.remove(linesSeen.keys.first())
        return true
    }
}
