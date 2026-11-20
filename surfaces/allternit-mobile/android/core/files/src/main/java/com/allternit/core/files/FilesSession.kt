package com.allternit.core.files

import android.content.Context

/**
 * Files sign-in from the native session (gap run GS-B). Every cloud-api route the Files client calls
 * (`/api/v1/files-keys/*`, `/api/v1/runtime-devices/:id/proxy` for Files v2, `/api/v1/computer-files/*`,
 * `/api/v1/files-gateway/*`) authenticates with `auth::resolve_user_scoped(…, "compute")`, which accepts the normal
 * Clerk session JWT. So the native app needs no separate Files token and no Custom Tab hand-off: it gives the Files
 * client its own session here, and :app's FilesAuth uses it before the sealed Custom Tab token.
 *
 * The play and phonelink (TWA) builds never set [bearer], so they keep the Custom Tab sign-in.
 */
object FilesSession {
    /** A fresh Clerk session JWT, or null when signed out. Blocking: called on the Files client's I/O threads. */
    @Volatile var bearer: (() -> String?)? = null

    /** The native app's launcher activity (:feature:shell). */
    private const val NATIVE_MAIN = "com.allternit.feature.shell.MainActivity"

    /**
     * True in the nativeapp flavor, even before MainActivity ran in this process (the DocumentsProvider can start
     * the process on its own): the launcher entry is the native MainActivity.
     */
    fun isNativeApp(context: Context): Boolean = bearer != null ||
        context.packageManager.getLaunchIntentForPackage(context.packageName)?.component?.className == NATIVE_MAIN
}
