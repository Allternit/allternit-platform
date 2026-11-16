package com.allternit.app.files

import com.allternit.files.contract.Wire
import org.json.JSONArray
import org.json.JSONObject
import java.lang.reflect.Constructor
import java.lang.reflect.ParameterizedType
import java.lang.reflect.Type
import java.lang.reflect.WildcardType
import java.util.concurrent.ConcurrentHashMap

/**
 * JSON for the generated contract types (contracts/files-drive/gen/FilesDriveContract.gen.kt)
 * with org.json and plain Java reflection: the app has no serialization library and no
 * kotlin-reflect. Each generated type is a data class whose primary-constructor
 * parameters carry `@Wire("<json name>")`; enums carry their wire string in `wire`.
 *
 *  - decode: the constructor that takes one parameter per `@Wire` field. A missing
 *    required field fails in Kotlin's own null check (NullPointerException), so a bad
 *    response is an error, never a half-filled object.
 *  - encode: `componentN()` gives the N-th constructor value. Null optionals are left
 *    out (the schemas are closed and do not allow `null`).
 *
 * Sealed unions (FeedMessage, FilesEvent) are not decoded here: the Android client
 * uses the poll feed, not the socket.
 */
internal object WireJson {
    private class Shape(val ctor: Constructor<*>, val names: Array<String>, val types: Array<Type>)

    private val shapes = ConcurrentHashMap<Class<*>, Shape>()

    private fun shape(cls: Class<*>): Shape = shapes.getOrPut(cls) {
        val ctor = cls.declaredConstructors
            .filter { c -> c.parameterAnnotations.isNotEmpty() && c.parameterAnnotations.all { a -> a.any { it is Wire } } }
            .maxByOrNull { it.parameterCount }
            ?: error("${cls.name} has no @Wire constructor")
        ctor.isAccessible = true
        val names = Array(ctor.parameterCount) { i ->
            ctor.parameterAnnotations[i].filterIsInstance<Wire>().first().name
        }
        Shape(ctor, names, ctor.genericParameterTypes)
    }

    fun <T> decode(text: String, cls: Class<T>): T = decodeObject(JSONObject(text), cls)

    @Suppress("UNCHECKED_CAST")
    fun <T> decodeObject(obj: JSONObject, cls: Class<T>): T {
        val s = shape(cls)
        val args = Array<Any?>(s.names.size) { i ->
            if (!obj.has(s.names[i]) || obj.isNull(s.names[i])) null else value(obj.get(s.names[i]), s.types[i])
        }
        return try {
            s.ctor.newInstance(*args) as T
        } catch (e: java.lang.reflect.InvocationTargetException) {
            throw IllegalArgumentException("Bad ${cls.simpleName}: ${e.targetException.message}", e.targetException)
        }
    }

    private fun value(raw: Any, type: Type): Any? {
        val t = if (type is WildcardType) type.upperBounds[0] else type
        if (t is ParameterizedType) {
            val owner = t.rawType as Class<*>
            if (List::class.java.isAssignableFrom(owner)) {
                val arr = raw as JSONArray
                return List(arr.length()) { value(arr.get(it), t.actualTypeArguments[0]) }
            }
            if (Map::class.java.isAssignableFrom(owner)) {
                val o = raw as JSONObject
                val out = LinkedHashMap<String, Any?>()
                for (k in o.keys()) out[k] = if (o.isNull(k)) null else value(o.get(k), t.actualTypeArguments[1])
                return out
            }
        }
        val cls = t as Class<*>
        return when {
            cls == String::class.java -> raw.toString()
            cls == java.lang.Long.TYPE || cls == java.lang.Long::class.java -> (raw as Number).toLong()
            cls == java.lang.Integer.TYPE || cls == java.lang.Integer::class.java -> (raw as Number).toInt()
            cls == java.lang.Double.TYPE || cls == java.lang.Double::class.java -> (raw as Number).toDouble()
            cls == java.lang.Boolean.TYPE || cls == java.lang.Boolean::class.java -> raw as Boolean
            cls.isEnum -> enumFromWire(cls, raw.toString())
            cls == Any::class.java -> raw
            else -> decodeObject(raw as JSONObject, cls)
        }
    }

    private fun enumFromWire(cls: Class<*>, wire: String): Any {
        val getter = cls.getMethod("getWire")
        return cls.enumConstants.firstOrNull { getter.invoke(it) == wire }
            ?: throw IllegalArgumentException("Unknown ${cls.simpleName} value: $wire")
    }

    fun encode(value: Any): JSONObject {
        val s = shape(value.javaClass)
        val obj = JSONObject()
        for (i in s.names.indices) {
            val v = value.javaClass.getMethod("component${i + 1}").invoke(value) ?: continue
            obj.put(s.names[i], jsonValue(v))
        }
        return obj
    }

    private fun jsonValue(v: Any): Any = when (v) {
        is String, is Boolean, is Long, is Int, is Double -> v
        is Enum<*> -> v.javaClass.getMethod("getWire").invoke(v) as String
        is List<*> -> JSONArray().also { arr -> v.forEach { arr.put(if (it == null) JSONObject.NULL else jsonValue(it)) } }
        is Map<*, *> -> JSONObject().also { o -> v.forEach { (k, x) -> if (x != null) o.put(k.toString(), jsonValue(x)) } }
        else -> encode(v)
    }
}

/** Shorthand: `json.wire<ListResponse>()`. */
internal inline fun <reified T> String.wire(): T = WireJson.decode(this, T::class.java)
