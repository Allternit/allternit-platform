package com.allternit.ondevice

import org.json.JSONArray

/**
 * Which phones may run models on the GPU. ggml-opencl's kernels are written for Adreno 7xx and
 * newer (Snapdragon 8 Gen 1 and up, 7+ Gen 3). On older Adreno they fail inside the driver
 * (Moto g play 2024, SM6225 / Adreno 610: clEnqueueNDRangeKernel error -54 on the first reply),
 * and ggml_abort() kills the process, so a failed GPU run cannot be caught and retried on the
 * CPU. Unknown or older SoCs therefore get the CPU. Allow-list, never block-list.
 */
object GpuPolicy {
    private val ADRENO_7XX_PLUS = Regex("^SM(84[5-9]\\d|8[5-9]\\d\\d|7675)$")

    fun openclAllowed(socManufacturer: String?, socModel: String?): Boolean =
        socManufacturer.equals("QTI", ignoreCase = true) && socModel != null && ADRENO_7XX_PLUS.matches(socModel)

    /** Drops GPU devices the policy does not allow, so the engine and the page never see them. */
    fun filter(devices: JSONArray, allowed: Boolean): JSONArray {
        if (allowed) return devices
        val out = JSONArray()
        for (i in 0 until devices.length()) {
            val d = devices.getJSONObject(i)
            val t = d.optString("type")
            if (t != "gpu" && t != "igpu") out.put(d)
        }
        return out
    }
}
