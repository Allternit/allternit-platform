package com.allternit.feature.office.sign

import android.util.Base64
import com.allternit.core.api.ApiCall
import com.allternit.core.api.ApiJson
import com.allternit.core.api.ApiResult
import com.allternit.core.api.gen.AllternitApi
import com.allternit.core.api.gen.CreateSignRequestBody
import com.allternit.core.api.gen.DeclineBody
import com.allternit.core.api.gen.SentBody
import com.allternit.core.api.gen.SignBody
import com.allternit.core.api.gen.SignDetail
import com.allternit.core.api.gen.SignRequest
import com.allternit.core.api.gen.SignRequestList
import com.allternit.core.api.gen.SignSendTo
import com.allternit.core.api.gen.SignSlot
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.str
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import okhttp3.MediaType.Companion.toMediaTypeOrNull
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import java.security.MessageDigest

/** A file in the account's cloud files, as the sign routes want it (id + SHA-256 of the exact bytes uploaded). */
data class UploadedFile(val fileId: String, val sha256: String)

/**
 * Sign requests for Android (iOS SignRequestsClient): the generated [AllternitApi] sign calls, plus the cloud-files upload the
 * request and the signed copy ride on, and SHA-256 hex. Every call returns [ApiResult]; nothing throws to the caller.
 * Handlers live in cmd/allternit-cloud-api (sign_requests.rs); the contract is in handoffs/review-sign-2026-11-26/PLAN.md.
 */
object SignRepository {
    /** Consent text v1 (Design decision 7), word for word as on iOS and web. */
    const val CONSENT_TEXT =
        "By tapping Sign & send, you agree to sign this document electronically. This adds an image of your signature and a signing record. It is not a certificate-based digital signature."
    const val CONSENT_VERSION = "v1"
    const val MAX_SIGNATURE_BYTES = 256 * 1024
    const val SURFACE = "android"

    private val api: AllternitApi by lazy { AllternitApi(LibraryNet.okHttp, LibraryNet.config) }

    fun sha256Hex(bytes: ByteArray): String =
        MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }

    suspend fun list(status: String? = null, threadId: String? = null, limit: Long? = null): ApiResult<SignRequestList> =
        api.listSignRequests(status, threadId, limit)

    suspend fun get(id: String): ApiResult<SignRequest> = api.getSignRequest(id)

    suspend fun create(body: CreateSignRequestBody): ApiResult<SignRequest> = api.createSignRequest(body)

    suspend fun sign(id: String, body: SignBody): ApiResult<SignRequest> = api.signSignRequest(id, body)

    suspend fun decline(id: String, reason: String? = null): ApiResult<SignRequest> = api.declineSignRequest(id, DeclineBody(reason))

    suspend fun cancel(id: String): ApiResult<SignRequest> = api.cancelSignRequest(id)

    suspend fun markSent(id: String, to: String): ApiResult<SignRequest> = api.markSignRequestSent(id, SentBody(to))

    /** The stored signature PNG (owner only). The generated call returns Unit, so this fetches the bytes by hand. */
    suspend fun signaturePng(id: String): ByteArray? = fetchBytes(signUrl(id) + "/signature", "image/png")

    /** The document or signed copy behind a request's 10-minute link. */
    suspend fun download(url: String): ByteArray? = fetchBytes(url, "*/*")

    private fun signUrl(id: String): String =
        LibraryNet.config.cloudBaseUrl.newBuilder().addPathSegments("api/v1/sign-requests").addPathSegment(id).build().toString()

    private suspend fun fetchBytes(url: String, accept: String): ByteArray? = withContext(Dispatchers.IO) {
        runCatching {
            // Signed links carry their own auth; ours attaches the bearer to everything, which a presigned host ignores.
            val client: OkHttpClient = if (url.startsWith(LibraryNet.config.cloudBaseUrl.toString().trimEnd('/'))) LibraryNet.okHttp else OkHttpClient()
            client.newCall(Request.Builder().url(url).header("Accept", accept).build()).execute().use { r ->
                if (r.isSuccessful) r.body?.bytes() else null
            }
        }.getOrNull()
    }

    /**
     * Cloud files upload (ArtifactUpload CloudFiles): `POST api/v1/files/uploads` → PUT the bytes to `putUrl` → complete.
     * Returns the file id and the SHA-256 of [bytes], or a failure with a human message.
     */
    suspend fun upload(bytes: ByteArray, name: String, mime: String): Result<UploadedFile> = withContext(Dispatchers.IO) {
        fun url(path: String) = LibraryNet.config.cloudBaseUrl.newBuilder().addPathSegments(path).build()
        val meta = buildJsonObject { put("name", name); put("contentType", mime); put("bytes", bytes.size) }.toString()
        val start = ApiCall.execute(LibraryNet.okHttp, ApiJson.default, ApiCall.request("POST", url("api/v1/files/uploads"), meta), JsonElement.serializer())
        if (start is ApiResult.HttpError && start.status == 413) return@withContext Result.failure<UploadedFile>(IllegalStateException("Your cloud storage is full."))
        val o = (start as? ApiResult.Ok)?.value.obj
        val id = o.str("fileId")
        val put = o.str("putUrl")
        if (id == null || put == null) return@withContext Result.failure<UploadedFile>(IllegalStateException("Couldn't upload the document. Try again."))
        val ok = runCatching {
            OkHttpClient().newCall(Request.Builder().url(put).put(bytes.toRequestBody(mime.toMediaTypeOrNull())).build()).execute().use { it.isSuccessful }
        }.getOrDefault(false)
        if (!ok) return@withContext Result.failure<UploadedFile>(IllegalStateException("Couldn't upload the document. Try again."))
        val done = ApiCall.execute(LibraryNet.okHttp, ApiJson.default, ApiCall.request("POST", url("api/v1/files/${LibraryNet.escape(id)}/complete"), meta), JsonElement.serializer())
        if (done !is ApiResult.Ok) return@withContext Result.failure<UploadedFile>(IllegalStateException("Couldn't upload the document. Try again."))
        Result.success(UploadedFile(id, sha256Hex(bytes)))
    }

    /**
     * Self-sign: uploads [pdf] and creates a `self` request for it (the PDF editor path), pending until [submitSignature].
     * [slots] are in PDF points, origin bottom-left; empty means the signer places it.
     */
    suspend fun createSelf(
        title: String,
        pdf: ByteArray,
        fileName: String,
        slots: List<SignSlot> = emptyList(),
        summary: String? = null,
        details: List<SignDetail> = emptyList(),
        sendTo: SignSendTo? = null,
    ): Result<SignRequest> {
        val file = upload(pdf, fileName, "application/pdf").getOrElse { return Result.failure(it) }
        return when (val r = create(
            CreateSignRequestBody(
                title = title, summary = summary, details = details.take(8).ifEmpty { null },
                documentFileId = file.fileId, documentSha256 = file.sha256, slots = slots.ifEmpty { null }, sendTo = sendTo,
                requesterKind = "self", sourceSurface = SURFACE,
            ),
        )) {
            is ApiResult.Ok -> Result.success(r.value)
            else -> Result.failure(IllegalStateException(errorMessage(r)))
        }
    }

    /**
     * Finishes a request: uploads the composed [signedPdf], then `POST /sign` with the PNG, the slot it landed in and both
     * hashes. [documentSha256] is the hash of the original document (the server compares it to the stored one).
     */
    suspend fun submitSignature(
        request: SignRequest,
        signedPdf: ByteArray,
        png: ByteArray,
        method: String,
        slot: SignSlot,
        signedName: String = "signed-" + request.documentName,
    ): Result<SignRequest> {
        val signed = upload(signedPdf, signedName, "application/pdf").getOrElse { return Result.failure(it) }
        val body = SignBody(
            consent = true, consentVersion = CONSENT_VERSION, method = method,
            signaturePngBase64 = Base64.encodeToString(png, Base64.NO_WRAP),
            slot = slot, documentSha256 = request.documentSha256,
            signedFileId = signed.fileId, signedSha256 = signed.sha256, surface = SURFACE,
        )
        return when (val r = sign(request.id, body)) {
            is ApiResult.Ok -> Result.success(r.value)
            else -> Result.failure(IllegalStateException(errorMessage(r)))
        }
    }

    /** One line a person can act on, for any failed call. */
    fun errorMessage(r: ApiResult<*>): String = when (r) {
        is ApiResult.Ok -> ""
        is ApiResult.Network -> "You're offline. Check your connection and try again."
        is ApiResult.Decode -> "Allternit sent something this app version can't read. Update the app and try again."
        is ApiResult.HttpError -> when {
            r.status == 410 || r.code == "expired" -> "This request has expired."
            r.status == 409 && r.code == "document-changed" -> "The document changed after this request was made. Ask for a new one."
            r.status == 409 -> "This request was already signed, declined or cancelled."
            r.status == 403 -> "Sign in to your account on this device to sign."
            r.status == 404 -> "This request or document is no longer available."
            r.status == 400 && r.code == "bad-slot" -> "That signing spot isn't on the page. Tap where to sign."
            r.status == 400 -> "The signature could not be accepted. Try again."
            else -> r.message ?: "Something went wrong. Try again."
        }
    }
}
