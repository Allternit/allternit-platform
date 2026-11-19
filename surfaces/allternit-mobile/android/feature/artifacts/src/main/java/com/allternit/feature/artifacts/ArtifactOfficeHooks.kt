package com.allternit.feature.artifacts

import androidx.compose.runtime.Composable

/**
 * What the artifacts screens open from :feature:office, which depends on this module (so it can't be
 * imported here). :feature:shell sets these once at startup (`OfficeFeature.install()`); a null hook hides its entry.
 */
object ArtifactOfficeHooks {
    /** iOS PDFSignView(sourceURL:): title, the PDF's bytes, close. */
    var signPdf: (@Composable (title: String, pdf: ByteArray, onClose: () -> Unit) -> Unit)? = null

    /** iOS OfficeDocumentView(artifactId:) for a sectioned Office record (`origin.legacy_id`): id, close. */
    var legacyDocument: (@Composable (legacyId: String, onClose: () -> Unit) -> Unit)? = null

    /** iOS OfficeDocumentsView (the library's "Allternit Office" entry): close. */
    var documents: (@Composable (onClose: () -> Unit) -> Unit)? = null
}

/** The library's PDF upload for :feature:office (a signed PDF saved to the artifact library). Returns the new artifact's id. */
suspend fun uploadPdfToLibrary(context: android.content.Context, uri: android.net.Uri): Result<String> = ArtifactUploads.pdf(context, uri)
